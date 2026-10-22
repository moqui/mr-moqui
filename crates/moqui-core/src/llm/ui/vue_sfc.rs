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

//! The Vue SFC dialect (plans/LLM-A2A-Assist.md 4.10, J11, J12; docs/Assist.md "Vue SFC canvases"): a Vue 3
//! single-file component the agent writes when OpenUI Lang cannot express a layout, run in a sandboxed frame.
//!
//! The program is the component's blocks: `<template>`, `<script>` (`module.exports = {...}`, the Options API), an
//! optional `<style>` and an optional `<actions>`, a JSON list of the writes it may ask for. The server checks it
//! before anything is shown ([`VueSfc::prepare`]): the size, the blocks, the template compiled with the vendored Vue
//! compiler (`libs/vue/compiler-dom.cjs.js` in QuickJS, the one the qvt screens compile with), the script compiled
//! (not run) in QuickJS, the constructs the sandbox refuses (the page, the network, navigation, code from text, raw
//! HTML, elements that load or post), the widgets, the links and the actions. Each problem names its section of the
//! skill `vue-sfc` and quotes its rule, as OpenUI's do. These checks are a first layer that tells the model what does
//! not work; the boundary is the frame: the component is compiled in the browser, inside an opaque-origin frame
//! ([`frame_document`], served by the host at `GET /assist/sandbox` with its own CSP), whose only way out is the
//! parent's validated message port.
//!
//! A patch is a program with only the blocks that change: each replaces the canvas's block of its name. A write is an
//! action of the `<actions>` block, named by the frame's submit and run by the server from the stored program with
//! the values the user confirmed ([`action_request`], through [`super::write::hold`]): the fixed `parameters` and the
//! named `fields`, nothing else.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use rquickjs::{CatchResultExt, Context, Runtime};
use serde_json::{json, Map, Value as Json};

use super::openui::{Problem, Rule};
use super::{Dialect, Prepared, Refused, WriteInput};
use crate::context::ExecutionContextFactory;

/// The skill whose sections the errors name.
pub const SKILL: &str = "vue-sfc";
/// The longest program (or patch) taken, in characters.
pub const MAX_PROGRAM_CHARS: usize = 64 * 1024;
/// The most actions a component declares, fields and parameters an action has.
pub const MAX_ACTIONS: usize = 10;
pub const MAX_ACTION_VALUES: usize = 50;
/// The most problems one error lists.
const MAX_PROBLEMS: usize = 5;
/// The block names, in the order a program is written back.
pub const BLOCKS: [&str; 5] = ["template", "script", "style", "actions", "reads"];

macro_rules! rule {
    ($name:ident, $section:expr, $text:expr) => {
        pub const $name: Rule = Rule { skill: SKILL, section: $section, text: $text };
    };
}

rule!(
    SHAPE,
    "shape",
    "A program is one Vue 3 single-file component: a <template>, a <script> that sets module.exports = { ... }, an optional <style> and an optional <actions> block, and nothing else."
);
rule!(
    SCRIPT,
    "shape",
    "The script is plain JavaScript that sets module.exports to the Options API object: no import, no export default, no <script setup> and no TypeScript."
);
rule!(TEMPLATE, "shape", "The template is Vue 3 template syntax: every tag closes and every attribute value is quoted.");
rule!(
    SANDBOX,
    "sandbox",
    "The component has no page, no network and no navigation: never use window, document, location, fetch, XMLHttpRequest, WebSocket, eval, Function or import; read with this.$bridge.get and write with this.$bridge.submit."
);
rule!(
    HTML,
    "sandbox",
    "The template has no <script>, <style>, <iframe>, <object>, <embed>, <form>, <base>, <meta> or <link>, no v-html and no javascript: URL."
);
rule!(STYLE, "sandbox", "A <style> styles the component only: no @import, and url() only of a data: image.");
rule!(
    READ_CALL,
    "bridge",
    "Read with this.$bridge.get(path, query, list): path is an /apps screen with a list, or a transition that answers JSON, and the Promise gives its JSON."
);
rule!(
    WRITES,
    "bridge",
    "A write is an action of the <actions> block: this.$bridge.submit(name, values) from the user's click asks the user to confirm it, and the server runs it."
);
rule!(
    ACTIONS,
    "bridge",
    "<actions> is a JSON list of {name, label, path, fields, parameters}: path is a transition of an /apps screen, fields the values a submit sends, parameters fixed values, and no secret."
);
rule!(
    LINKS,
    "bridge",
    "A link is <a href=\"/apps/...\"> to a screen of the apps, which opens beside the canvas; another site opens only when the user allows external content."
);
rule!(
    READS,
    "bridge",
    "Every path this.$bridge.get reads is declared in the <reads> block, a JSON list of /apps screen paths; a path not listed is refused."
);
rule!(
    WIDGETS,
    "widgets",
    "Use Quasar components (q-*) and the widgets listed here: m-text-line, m-drop-down, m-date-time, m-display, m-date-period and m-container-box; no other m- component and no router-link."
);

/// Every rule, for the test that each is quoted in its section.
pub const RULES: &[Rule] = &[SHAPE, SCRIPT, TEMPLATE, SANDBOX, HTML, STYLE, READ_CALL, READS, WRITES, ACTIONS, LINKS, WIDGETS];

/// The widgets the frame provides (`js/WebrootAssistSandbox.js`).
pub const WIDGET_NAMES: &[&str] = &["m-text-line", "m-drop-down", "m-date-time", "m-display", "m-date-period", "m-container-box"];
/// Elements that load, post, navigate or run code.
const BANNED_ELEMENTS: &[&str] = &[
    "script", "style", "iframe", "frame", "frameset", "object", "embed", "applet", "form", "base", "meta", "link", "portal",
    "fencedframe",
];
/// Names that reach the page, the network, navigation or code from text, when they are not a property (`row.location`
/// is a field; `location` alone is the frame's).
const BANNED_GLOBALS: &[&str] = &[
    "window", "document", "globalThis", "self", "top", "parent", "frames", "opener", "location", "navigation", "navigator",
    "localStorage", "sessionStorage", "indexedDB", "caches", "cookieStore", "fetch", "XMLHttpRequest", "WebSocket",
    "WebTransport", "EventSource", "Worker", "SharedWorker", "importScripts", "eval", "Function", "require", "postMessage",
    "MessageChannel", "BroadcastChannel", "RTCPeerConnection", "webkitRTCPeerConnection", "RTCDataChannel",
];
/// Properties that lead out of the component's own DOM or turn text into markup or code, wherever they appear.
const BANNED_PROPERTIES: &[&str] = &[
    "ownerDocument", "defaultView", "contentWindow", "contentDocument", "constructor", "__proto__", "innerHTML",
    "outerHTML", "insertAdjacentHTML", "srcdoc", "createContextualFragment",
];
/// Attributes that load or post.
const BANNED_ATTRIBUTES: &[&str] = &["v-html", "srcdoc", "formaction", "http-equiv"];

/// The blocks of a program, each its content.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blocks {
    pub template: Option<String>,
    pub script: Option<String>,
    pub style: Option<String>,
    pub actions: Option<String>,
    pub reads: Option<String>,
}

impl Blocks {
    fn get(&self, name: &str) -> Option<&String> {
        match name {
            "template" => self.template.as_ref(),
            "script" => self.script.as_ref(),
            "style" => self.style.as_ref(),
            "actions" => self.actions.as_ref(),
            "reads" => self.reads.as_ref(),
            _ => None,
        }
    }

    fn slot(&mut self, name: &str) -> Option<&mut Option<String>> {
        match name {
            "template" => Some(&mut self.template),
            "script" => Some(&mut self.script),
            "style" => Some(&mut self.style),
            "actions" => Some(&mut self.actions),
            "reads" => Some(&mut self.reads),
            _ => None,
        }
    }

    /// The names of the blocks it has, in order.
    pub fn names(&self) -> Vec<String> {
        BLOCKS.iter().filter(|name| self.get(name).is_some()).map(|name| (*name).to_string()).collect()
    }

    /// The program written back: each block it has, in order, a blank line between.
    pub fn text(&self) -> String {
        BLOCKS
            .iter()
            .filter_map(|name| self.get(name).map(|content| format!("<{name}>\n{}\n</{name}>", content.trim_matches('\n'))))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// `self` with each block of `patch` in place of its own.
    pub fn patched(mut self, patch: &Blocks) -> Self {
        for name in BLOCKS {
            if let Some(content) = patch.get(name) {
                if let Some(slot) = self.slot(name) {
                    *slot = Some(content.clone());
                }
            }
        }
        self
    }
}

fn problem(rule: Rule, block: Option<&str>, line: usize, message: impl Into<String>) -> Problem {
    let mut out = Problem::new(rule, message);
    out.statement = block.map(str::to_string);
    out.line = line;
    out
}

/// The 1-based line of `offset` (a char index) in `chars`.
fn line_at(chars: &[char], offset: usize) -> usize {
    1 + chars.iter().take(offset).filter(|ch| **ch == '\n').count()
}

fn starts_with_at(chars: &[char], at: usize, text: &str) -> bool {
    let mut index = at;
    for expected in text.chars() {
        match chars.get(index) {
            Some(ch) if ch.eq_ignore_ascii_case(&expected) => index += 1,
            _ => return false,
        }
    }
    true
}

fn find_from(chars: &[char], from: usize, text: &str) -> Option<usize> {
    (from..chars.len()).find(|at| starts_with_at(chars, *at, text))
}

/// Whether `<name` at `at` opens a tag of that name (the name is followed by a space, `>` or `/`).
fn tag_at(chars: &[char], at: usize, name: &str) -> bool {
    starts_with_at(chars, at, &format!("<{name}"))
        && chars.get(at + name.len() + 1).is_none_or(|ch| ch.is_whitespace() || *ch == '>' || *ch == '/')
}

/// The blocks of `text`. The JavaScript twin is `canvas.sfc.blocks` in `js/WebrootAssistCanvas.js`: a top-level
/// block is `<template>`, `<script>`, `<style>` or `<actions>` (a nested `<template>` counts toward its own close),
/// and only white space and comments may be between them.
///
/// # Errors
///
/// The problem: an unknown or doubled block, text outside the blocks, a block that does not close, a `lang` or
/// `setup` script.
pub fn blocks(text: &str) -> Result<Blocks, Box<Problem>> {
    let out = split(text)?;
    if out.template.as_deref().is_none_or(|template| template.trim().is_empty()) {
        return Err(Box::new(problem(SHAPE, Some("template"), 0, "the program has no <template>")));
    }
    Ok(out)
}

/// The blocks of `text`, a program or a patch (which may leave the template out).
fn split(text: &str) -> Result<Blocks, Box<Problem>> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Blocks::default();
    let mut at = 0;
    while at < chars.len() {
        if chars[at].is_whitespace() {
            at += 1;
            continue;
        }
        if starts_with_at(&chars, at, "<!--") {
            at = find_from(&chars, at + 4, "-->").map_or(chars.len(), |end| end + 3);
            continue;
        }
        let Some(name) = BLOCKS.iter().find(|name| tag_at(&chars, at, name)) else {
            let line = line_at(&chars, at);
            let shown: String = chars[at..].iter().take(40).collect();
            return Err(Box::new(problem(SHAPE, None, line, format!("line {line} is outside the blocks: {}", shown.trim()))));
        };
        let line = line_at(&chars, at);
        let Some(open_end) = (at..chars.len()).find(|index| chars[*index] == '>') else {
            return Err(Box::new(problem(SHAPE, Some(name), line, format!("the <{name}> tag does not close"))));
        };
        let attributes: String = chars[at + name.len() + 1..open_end].iter().collect::<String>().to_ascii_lowercase();
        if *name == "script" && (attributes.contains("setup") || attributes.contains("lang")) {
            return Err(Box::new(problem(SCRIPT, Some(name), line, format!("<script{attributes}> is not plain JavaScript; write <script> with module.exports"))));
        }
        if *name == "template" && attributes.contains("lang") {
            return Err(Box::new(problem(TEMPLATE, Some(name), line, "the template is HTML with Vue syntax; it takes no lang")));
        }
        let start = open_end + 1;
        let close = format!("</{name}>");
        let end = if *name == "template" {
            let mut depth = 1;
            let mut index = start;
            let mut found = None;
            while index < chars.len() {
                if tag_at(&chars, index, "template") {
                    depth += 1;
                } else if starts_with_at(&chars, index, "</template") {
                    depth -= 1;
                    if depth == 0 {
                        found = Some(index);
                        break;
                    }
                }
                index += 1;
            }
            found
        } else {
            find_from(&chars, start, &close)
        };
        let Some(end) = end else {
            return Err(Box::new(problem(SHAPE, Some(name), line, format!("the <{name}> block has no {close}"))));
        };
        let content: String = chars[start..end].iter().collect();
        let slot = out.slot(name).expect("a block name");
        if slot.is_some() {
            return Err(Box::new(problem(SHAPE, Some(name), line, format!("there is a second <{name}> block"))));
        }
        *slot = Some(content);
        at = (end..chars.len()).find(|index| chars[*index] == '>').map_or(chars.len(), |index| index + 1);
    }
    Ok(out)
}

/// `source` with the text of its strings and comments blanked (the same length, line breaks kept), so a scan of the
/// rest sees only code; a template literal's `${...}` stays code.
pub fn code_only(source: &str) -> Vec<char> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = chars.clone();
    let blank = |out: &mut Vec<char>, index: usize| {
        if out[index] != '\n' {
            out[index] = ' ';
        }
    };
    // the brace depth of each open `${` of a template literal, innermost last
    let mut templates: Vec<usize> = Vec::new();
    let mut depth = 0_usize;
    let mut index = 0;
    let mut in_template = false;
    while index < chars.len() {
        let ch = chars[index];
        if in_template {
            if ch == '\\' {
                blank(&mut out, index);
                if index + 1 < chars.len() {
                    blank(&mut out, index + 1);
                }
                index += 2;
                continue;
            }
            if ch == '`' {
                in_template = false;
                index += 1;
                continue;
            }
            if ch == '$' && chars.get(index + 1) == Some(&'{') {
                templates.push(depth);
                depth += 1;
                in_template = false;
                index += 2;
                continue;
            }
            blank(&mut out, index);
            index += 1;
            continue;
        }
        match ch {
            '/' if chars.get(index + 1) == Some(&'/') => {
                while index < chars.len() && chars[index] != '\n' {
                    blank(&mut out, index);
                    index += 1;
                }
            }
            '/' if chars.get(index + 1) == Some(&'*') => {
                let end = find_from(&chars, index + 2, "*/").map_or(chars.len(), |end| end + 2);
                for at in index..end {
                    blank(&mut out, at);
                }
                index = end;
            }
            '\'' | '"' => {
                index += 1;
                while index < chars.len() && chars[index] != ch && chars[index] != '\n' {
                    if chars[index] == '\\' && index + 1 < chars.len() {
                        blank(&mut out, index);
                        index += 1;
                    }
                    blank(&mut out, index);
                    index += 1;
                }
                index += 1;
            }
            '`' => {
                in_template = true;
                index += 1;
            }
            '{' => {
                depth += 1;
                index += 1;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if templates.last() == Some(&depth) {
                    templates.pop();
                    in_template = true;
                }
                index += 1;
            }
            _ => index += 1,
        }
    }
    out
}

/// The problems of the code `source` (a script, or a template expression at `base_line` of `block`): the names the
/// sandbox refuses, `import` and `export`. In a template a free name is the component's own (Vue resolves it on the
/// instance, and only a few globals such as `Math` reach a template), so there only the properties are checked.
fn code_problems(source: &str, block: &str, base_line: usize, out: &mut Vec<Problem>) {
    let template = block == "template";
    let code = code_only(source);
    let is_start = |ch: char| ch.is_ascii_alphabetic() || ch == '_' || ch == '$';
    let is_part = |ch: char| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$';
    let mut index = 0;
    while index < code.len() {
        if !is_start(code[index]) || (index > 0 && is_part(code[index - 1])) {
            index += 1;
            continue;
        }
        let start = index;
        while index < code.len() && is_part(code[index]) {
            index += 1;
        }
        let name: String = code[start..index].iter().collect();
        let before = code[..start].iter().rev().find(|ch| !ch.is_whitespace()).copied();
        let after = code[index..].iter().find(|ch| !ch.is_whitespace()).copied();
        let property = before == Some('.');
        let key = after == Some(':') && code.get(index + 1) != Some(&':');
        let line = base_line + line_at(&code, start) - 1;
        if BANNED_PROPERTIES.contains(&name.as_str()) {
            out.push(problem(SANDBOX, Some(block), line, format!("{name} leads out of the component")));
        } else if !template && !property && !key && BANNED_GLOBALS.contains(&name.as_str()) {
            out.push(problem(SANDBOX, Some(block), line, format!("{name} is not there in the sandbox")));
        } else if !template && !property && !key && (name == "import" || name == "export") {
            out.push(problem(SCRIPT, Some(block), line, format!("{name} is not plain JavaScript here; set module.exports")));
        }
    }
}

/// A tag of a template: its name, its attributes (name and value) and its line.
struct Tag {
    name: String,
    attributes: Vec<(String, Option<String>)>,
    line: usize,
}

/// The tags and the `{{ }}` expressions (with their lines) of `template`.
fn template_parts(template: &str) -> (Vec<Tag>, Vec<(String, usize)>) {
    let chars: Vec<char> = template.chars().collect();
    let mut tags = Vec::new();
    let mut expressions = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if starts_with_at(&chars, index, "<!--") {
            index = find_from(&chars, index + 4, "-->").map_or(chars.len(), |end| end + 3);
            continue;
        }
        if starts_with_at(&chars, index, "{{") {
            let end = find_from(&chars, index + 2, "}}").unwrap_or(chars.len());
            expressions.push((chars[index + 2..end].iter().collect(), line_at(&chars, index)));
            index = end + 2;
            continue;
        }
        if chars[index] == '<' && chars.get(index + 1).is_some_and(char::is_ascii_alphabetic) {
            let line = line_at(&chars, index);
            let mut at = index + 1;
            let name_start = at;
            while at < chars.len() && !chars[at].is_whitespace() && chars[at] != '>' && chars[at] != '/' {
                at += 1;
            }
            let name: String = chars[name_start..at].iter().collect::<String>().to_ascii_lowercase();
            let mut attributes = Vec::new();
            loop {
                while at < chars.len() && (chars[at].is_whitespace() || chars[at] == '/') {
                    at += 1;
                }
                if at >= chars.len() || chars[at] == '>' {
                    break;
                }
                let attr_start = at;
                while at < chars.len() && !chars[at].is_whitespace() && chars[at] != '=' && chars[at] != '>' && chars[at] != '/' {
                    at += 1;
                }
                let attr: String = chars[attr_start..at].iter().collect();
                let mut value = None;
                if chars.get(at) == Some(&'=') {
                    at += 1;
                    match chars.get(at) {
                        Some(quote @ ('"' | '\'')) => {
                            let quote = *quote;
                            let value_start = at + 1;
                            at = value_start;
                            while at < chars.len() && chars[at] != quote {
                                at += 1;
                            }
                            value = Some(chars[value_start..at.min(chars.len())].iter().collect());
                            at += 1;
                        }
                        _ => {
                            let value_start = at;
                            while at < chars.len() && !chars[at].is_whitespace() && chars[at] != '>' {
                                at += 1;
                            }
                            value = Some(chars[value_start..at].iter().collect());
                        }
                    }
                }
                if attr.is_empty() {
                    at += 1;
                } else {
                    attributes.push((attr, value));
                }
            }
            tags.push(Tag { name, attributes, line });
            index = at + 1;
            continue;
        }
        index += 1;
    }
    (tags, expressions)
}

/// Whether an attribute is a binding whose value is code (`:x`, `@x`, `#x`, `v-…`).
fn is_binding(name: &str) -> bool {
    name.starts_with(':') || name.starts_with('@') || name.starts_with('#') || name.starts_with("v-")
}

fn template_problems(template: &str, out: &mut Vec<Problem>) {
    let (tags, expressions) = template_parts(template);
    for tag in &tags {
        if BANNED_ELEMENTS.contains(&tag.name.as_str()) {
            out.push(problem(HTML, Some("template"), tag.line, format!("<{}> is not allowed in the template", tag.name)));
        } else if (tag.name.starts_with("m-") && !WIDGET_NAMES.contains(&tag.name.as_str())) || tag.name.starts_with("router-") {
            out.push(problem(WIDGETS, Some("template"), tag.line, format!("<{}> is not one of the widgets of the sandbox", tag.name)));
        }
        for (name, value) in &tag.attributes {
            let lower = name.to_ascii_lowercase();
            if BANNED_ATTRIBUTES.contains(&lower.as_str()) {
                out.push(problem(HTML, Some("template"), tag.line, format!("the attribute {name} is not allowed")));
                continue;
            }
            let Some(value) = value else { continue };
            let squeezed: String = value.chars().filter(|ch| !ch.is_whitespace()).collect::<String>().to_ascii_lowercase();
            if squeezed.contains("javascript:") || squeezed.contains("vbscript:") {
                out.push(problem(HTML, Some("template"), tag.line, format!("{name} has a javascript: URL")));
            } else if is_binding(name) {
                code_problems(value, "template", tag.line, out);
            } else if (lower == "href" || lower == "to") && value.trim_start().starts_with("//") {
                out.push(problem(LINKS, Some("template"), tag.line, format!("{value} is a link to another site without its scheme")));
            }
        }
    }
    for (expression, line) in &expressions {
        code_problems(expression, "template", *line, out);
    }
}

fn style_problems(style: &str, out: &mut Vec<Problem>) {
    let lower = style.to_ascii_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    if let Some(at) = find_from(&chars, 0, "@import") {
        out.push(problem(STYLE, Some("style"), line_at(&chars, at), "@import loads a stylesheet"));
    }
    let mut from = 0;
    while let Some(at) = find_from(&chars, from, "url(") {
        let rest: String = chars[at + 4..].iter().take(12).collect();
        let rest = rest.trim_start().trim_start_matches(['"', '\'']);
        if !rest.starts_with("data:") {
            out.push(problem(STYLE, Some("style"), line_at(&chars, at), "url() loads a file; use a data: image"));
            break;
        }
        from = at + 4;
    }
}

/// An action of the `<actions>` block.
#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub name: String,
    pub label: String,
    pub method: String,
    pub path: String,
    pub fields: Vec<String>,
    pub parameters: Map<String, Json>,
}

fn plain_name(name: &str, max: usize) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        && name.chars().count() <= max
        && name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

/// The actions of `text` (an `<actions>` block), checked: a JSON list of at most [`MAX_ACTIONS`] objects with a plain
/// unique `name`, a `path` that is a transition path of the apps, `fields` (names) and `parameters` (fixed scalars),
/// none secret.
///
/// # Errors
///
/// What is wrong, for the [`ACTIONS`] rule.
pub fn parse_actions(text: &str) -> Result<Vec<Action>, String> {
    let list: Json = serde_json::from_str(text.trim()).map_err(|error| format!("the actions are not JSON: {error}"))?;
    let Json::Array(items) = list else {
        return Err("the actions are not a JSON list".to_string());
    };
    if items.len() > MAX_ACTIONS {
        return Err(format!("there are {} actions, more than {MAX_ACTIONS}", items.len()));
    }
    let mut out: Vec<Action> = Vec::new();
    for item in &items {
        let Json::Object(map) = item else {
            return Err("an action is not an object".to_string());
        };
        let name = map.get("name").and_then(Json::as_str).unwrap_or_default().to_string();
        if !plain_name(&name, 40) {
            return Err(format!("the action name {name:?} is not a plain name (letters, digits and _)"));
        }
        if out.iter().any(|other| other.name == name) {
            return Err(format!("there are two actions named {name}"));
        }
        if let Some(key) = map.keys().find(|key| !["name", "label", "method", "path", "fields", "parameters"].contains(&key.as_str())) {
            return Err(format!("the action {name} has {key}, which an action does not take"));
        }
        let label = map.get("label").and_then(Json::as_str).unwrap_or(&name).chars().take(80).collect::<String>();
        let method = map.get("method").and_then(Json::as_str).unwrap_or("POST").to_ascii_uppercase();
        if !["POST", "PUT", "PATCH", "DELETE"].contains(&method.as_str()) {
            return Err(format!("the action {name} uses {method}; an action writes with POST"));
        }
        let path = crate::llm::tools::apps::app_path(map.get("path").and_then(Json::as_str).unwrap_or_default());
        if !path.starts_with("/apps/") || path.contains('?') || path.contains('#') {
            return Err(format!("the action {name} has the path {path:?}, not a transition of an /apps screen"));
        }
        if let Some(problem) = crate::llm::screen::path_problem(&path) {
            return Err(format!("the action {name} has the path {path}: {problem}"));
        }
        let fields: Vec<String> = match map.get("fields") {
            None | Some(Json::Null) => Vec::new(),
            Some(Json::Array(fields)) => fields.iter().map(|field| field.as_str().unwrap_or_default().to_string()).collect(),
            Some(_) => return Err(format!("the fields of the action {name} are not a list of names")),
        };
        let parameters = match map.get("parameters") {
            None | Some(Json::Null) => Map::new(),
            Some(Json::Object(parameters)) => parameters.clone(),
            Some(_) => return Err(format!("the parameters of the action {name} are not an object")),
        };
        if fields.len() + parameters.len() > MAX_ACTION_VALUES {
            return Err(format!("the action {name} sends more than {MAX_ACTION_VALUES} values"));
        }
        let mut seen = BTreeSet::new();
        for field in fields.iter().chain(parameters.keys()) {
            if !plain_name(field, 100) {
                return Err(format!("the action {name} sends {field:?}, which is not a plain name"));
            }
            if crate::llm::tools::data::secret_name(field) {
                return Err(format!("the action {name} sends {field}, a secret; a canvas does not post secrets"));
            }
            if !seen.insert(field.clone()) {
                return Err(format!("the action {name} sends {field} twice"));
            }
        }
        if let Some((key, _)) = parameters.iter().find(|(_, value)| value.is_object() || value.is_array()) {
            return Err(format!("the parameter {key} of the action {name} is not a plain value"));
        }
        out.push(Action { name, label, method, path, fields, parameters });
    }
    Ok(out)
}

/// The declared read paths of a `<reads>` block (`text`): a JSON list of `/apps` screen paths (a string, or an object
/// with a `path`), each normalized to its `/apps` path with no query. The read proxy answers only these paths.
///
/// # Errors
///
/// What is wrong, for the [`READS`] rule.
pub fn parse_reads(text: &str) -> Result<Vec<String>, String> {
    let list: Json = serde_json::from_str(text.trim()).map_err(|error| format!("the reads are not JSON: {error}"))?;
    let Json::Array(items) = list else {
        return Err("the reads are not a JSON list".to_string());
    };
    if items.len() > 50 {
        return Err(format!("there are {} reads, more than 50", items.len()));
    }
    let mut out: Vec<String> = Vec::new();
    for item in &items {
        let raw = match item {
            Json::String(path) => path.as_str(),
            Json::Object(map) => map.get("path").and_then(Json::as_str).unwrap_or_default(),
            _ => return Err("a read is not a path or an object with a path".to_string()),
        };
        let path = crate::llm::tools::apps::app_path(raw);
        let bare = path.split('?').next().unwrap_or_default().to_string();
        if !bare.starts_with("/apps/") || bare.contains("..") || bare.contains("//") {
            return Err(format!("the read {raw:?} is not an /apps screen path"));
        }
        if let Some(problem) = crate::llm::screen::path_problem(&bare) {
            return Err(format!("the read {bare}: {problem}"));
        }
        if !out.contains(&bare) {
            out.push(bare);
        }
    }
    Ok(out)
}

/// Whether the read `path` (its `/apps` path, no query) is one the program declared.
pub fn read_declared(declared: &[String], path: &str) -> bool {
    let bare = crate::llm::tools::apps::app_path(path);
    let bare = bare.split('?').next().unwrap_or_default();
    declared.iter().any(|allowed| allowed == bare)
}

/// Whether a reported problem of `source`/`code` is a fixed-code reason the server auto-revalidates: a compile error
/// of the template or the script, or a sandbox kill the watchdog named (its codes are fixed). A free-text runtime
/// error, a read or link refusal or anything else is never auto-revalidated (its message is the frame's to write).
pub fn auto_revalidates(source: &str, code: &str) -> bool {
    matches!(source, "compile") && matches!(code, "template" | "script")
        || matches!(source, "sandbox") && matches!(code, "navigation" | "timeout" | "unresponsive" | "flood")
}

/// The action names `$bridge.submit("…"` uses in `text`.
fn submitted_names(text: &str) -> Vec<(String, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = find_from(&chars, from, "$bridge.submit(") {
        let mut index = at + "$bridge.submit(".len();
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        if let Some(quote @ ('"' | '\'' | '`')) = chars.get(index) {
            let quote = *quote;
            let start = index + 1;
            let end = (start..chars.len()).find(|end| chars[*end] == quote).unwrap_or(chars.len());
            out.push((chars[start..end].iter().collect(), line_at(&chars, at)));
        }
        from = at + 1;
    }
    out
}

/// The syntax error of `script` compiled (not run) as the body of a function of `module` and `exports`, in a QuickJS
/// runtime of its own; `None` when it compiles, or when QuickJS cannot start (logged: the browser compiles it again).
fn script_error(script: &str) -> Option<String> {
    let started = (|| -> Result<(Runtime, Context), rquickjs::Error> {
        let runtime = Runtime::new()?;
        runtime.set_memory_limit(16 * 1024 * 1024);
        runtime.set_max_stack_size(1024 * 1024);
        let deadline = Instant::now() + Duration::from_secs(2);
        runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
        let context = Context::full(&runtime)?;
        Ok((runtime, context))
    })();
    let (_runtime, context) = match started {
        Ok(started) => started,
        Err(error) => {
            tracing::error!("QuickJS did not start to check a Vue SFC script: {error}");
            return None;
        }
    };
    let source = serde_json::to_string(script).ok()?;
    let check = format!(
        "(function () {{ try {{ new Function('module', 'exports', {source}); return ''; }} \
         catch (e) {{ return String((e && e.message) || e) + (e && e.lineNumber ? ' (line ' + e.lineNumber + ')' : ''); }} }})()"
    );
    context.with(|ctx| match ctx.eval::<String, _>(check).catch(&ctx) {
        Ok(text) if text.is_empty() => None,
        Ok(text) => Some(text),
        Err(error) => Some(error.to_string()),
    })
}

/// Check `blocks` and give its problems; `compiler` is the Vue compiler's source, when the server has it.
pub fn check(blocks: &Blocks, compiler: Option<&str>) -> Vec<Problem> {
    let mut out = Vec::new();
    let template = blocks.template.as_deref().unwrap_or_default();
    template_problems(template, &mut out);
    if let Some(compiler) = compiler {
        if let Err(error) = crate::screen::vue_check::compile_render(compiler, template) {
            let text = error.to_string();
            let line = text
                .rsplit(" at ")
                .next()
                .and_then(|at| at.split(':').next())
                .and_then(|line| line.trim().parse::<usize>().ok())
                .unwrap_or(0);
            out.push(problem(TEMPLATE, Some("template"), line, crate::llm::tools::format::cut(&text, 300)));
        }
    }
    let actions = match blocks.actions.as_deref() {
        Some(text) => match parse_actions(text) {
            Ok(actions) => actions,
            Err(why) => {
                out.push(problem(ACTIONS, Some("actions"), 0, why));
                Vec::new()
            }
        },
        None => Vec::new(),
    };
    if let Some(script) = blocks.script.as_deref().filter(|script| !script.trim().is_empty()) {
        code_problems(script, "script", 1, &mut out);
        if !script.contains("module.exports") {
            out.push(problem(SCRIPT, Some("script"), 0, "the script does not set module.exports"));
        } else if let Some(error) = script_error(script) {
            out.push(problem(SCRIPT, Some("script"), 0, format!("the script does not compile: {error}")));
        }
    }
    if let Some(style) = blocks.style.as_deref() {
        style_problems(style, &mut out);
    }
    if let Some(text) = blocks.reads.as_deref() {
        if let Err(why) = parse_reads(text) {
            out.push(problem(READS, Some("reads"), 0, why));
        }
    }
    for (block, text) in [("template", template), ("script", blocks.script.as_deref().unwrap_or_default())] {
        for (name, line) in submitted_names(text) {
            if blocks.actions.is_none() {
                out.push(problem(WRITES, Some(block), line, format!("submit names {name:?}, and there is no <actions> block")));
            } else if !actions.is_empty() && !actions.iter().any(|action| action.name == name) {
                let mut found = problem(WRITES, Some(block), line, format!("submit names {name:?}, which is not an action of the <actions> block"));
                found.did_you_mean = actions.iter().map(|action| action.name.clone()).collect();
                out.push(found);
            }
        }
    }
    out
}

/// The request of the action `name` of `program` with `state`: its method, path and body (the action's parameters,
/// then each of its fields from the state, a missing one null); `None` when it has no such action.
///
/// # Errors
///
/// When the stored program's actions do not read (they were checked when it was shown).
pub fn action_request(program: &str, name: &str, state: &Json) -> Result<Option<(String, String, Json)>, String> {
    let blocks = blocks(program).map_err(|problem| problem.message)?;
    let actions = match blocks.actions.as_deref() {
        Some(text) => parse_actions(text)?,
        None => Vec::new(),
    };
    let Some(action) = actions.into_iter().find(|action| action.name == name) else {
        return Ok(None);
    };
    let mut body = action.parameters.clone();
    for field in &action.fields {
        body.insert(field.clone(), state.get(field).cloned().unwrap_or(Json::Null));
    }
    Ok(Some((action.method, action.path, Json::Object(body))))
}

/// The error the model reads for `problems` (plan 7.4): the first problem's section to load and its rule quoted, its
/// block and line, and the others after it.
pub fn error_json(problems: &[Problem], patch: bool) -> String {
    let Some(first) = problems.first() else {
        return String::new();
    };
    let place = match (&first.statement, first.line) {
        (Some(block), line) if line > 0 => format!("Fix line {line} of the <{block}>"),
        (Some(block), _) => format!("Fix the <{block}>"),
        (None, line) if line > 0 => format!("Fix line {line}"),
        (None, _) => "Fix it".to_string(),
    };
    let resend = if patch { "send the changed blocks again" } else { "send the whole program again" };
    let mut error = crate::llm::tools::format::ToolError::new(
        "vue_sfc_invalid",
        format!(
            "The component has a problem, so nothing was shown: {} The rule is in skill {SKILL}, section {}; load it with skill name \"{}\" if it is not above. {place} and {resend}.",
            first.message,
            first.rule.section,
            first.skill()
        ),
    )
    .skill(first.skill())
    .rule(first.rule.text)
    .did_you_mean(first.did_you_mean.clone());
    if let Some(block) = &first.statement {
        error = error.with("block", json!(block));
    }
    if first.line > 0 {
        error = error.with("line", json!(first.line));
    }
    let also: Vec<Json> = problems
        .iter()
        .skip(1)
        .take(MAX_PROBLEMS - 1)
        .map(|problem| json!({"skill": problem.skill(), "rule": problem.rule.text, "problem": problem.message, "block": problem.statement}))
        .collect();
    if !also.is_empty() {
        error = error.with("also", Json::Array(also));
    }
    error.to_text()
}

/// A problem the frame or its parent reported after the canvas was drawn (`canvasProblems`), mapped to its section:
/// a compile error of the template or the script, an error while it ran, a widget Vue could not resolve, a read or a
/// submit the parent refused, a link it blocked, and the watchdog's kills (`navigation`, `flood`, `timeout`,
/// `unresponsive`), which are the sandbox's.
pub fn reported(source: &str, code: &str, message: &str, block: Option<&str>) -> Problem {
    let rule = match (source, code) {
        ("sandbox", _) => SANDBOX,
        ("compile", "template") => TEMPLATE,
        ("compile", _) => SCRIPT,
        (_, "component") => WIDGETS,
        ("read", _) => READ_CALL,
        ("submit", _) => WRITES,
        ("link", _) => LINKS,
        _ => SCRIPT,
    };
    let mut found = Problem::new(rule, crate::llm::tools::format::cut(message.trim(), 300));
    found.statement = block.map(str::to_string);
    found.source = source.to_string();
    found.code = code.to_string();
    found
}

/// The checks of a program and its merge with the canvas's (`previous`).
fn prepared(previous: Option<&str>, input: &WriteInput, compiler: Option<&str>) -> Result<Prepared, Vec<Problem>> {
    let text = input.program.as_deref().or(input.patch.as_deref()).unwrap_or_default();
    if text.chars().count() > MAX_PROGRAM_CHARS {
        return Err(vec![problem(SHAPE, None, 0, format!("the program is longer than {MAX_PROGRAM_CHARS} characters.")).parse_error()]);
    }
    let merged = match (&input.program, &input.patch, previous) {
        (None, Some(patch), Some(previous)) => {
            // a patch is the blocks that change; it need not have a template of its own
            let patch = split(patch).map_err(|problem| vec![*problem])?;
            let base = blocks(previous).map_err(|problem| vec![*problem])?;
            base.patched(&patch)
        }
        _ => blocks(text).map_err(|problem| vec![*problem])?,
    };
    let problems = check(&merged, compiler);
    if !problems.is_empty() {
        return Err(problems);
    }
    let mutations = merged
        .actions
        .as_deref()
        .and_then(|text| parse_actions(text).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|action| action.name)
        .collect();
    let reads = merged.reads.as_deref().and_then(|text| parse_reads(text).ok()).unwrap_or_default();
    Ok(Prepared { program: merged.text(), statements: merged.names(), mutations, reads })
}

#[derive(Default)]
pub struct VueSfc;

impl Dialect for VueSfc {
    fn name(&self) -> &'static str {
        "vue-sfc"
    }

    fn label(&self) -> &'static str {
        "Vue SFC"
    }

    fn skill(&self) -> &'static str {
        SKILL
    }

    fn prepare(
        &self,
        factory: &ExecutionContextFactory,
        previous: Option<&str>,
        input: &WriteInput,
        _paths: &dyn super::openui::Paths,
    ) -> Result<Prepared, Refused> {
        // the template compiles with the compiler the browser build has; without it the frame reports what does not
        let compiler = match crate::screen::statics::compiler_source(factory) {
            Ok(text) => Some(text),
            Err(error) => {
                tracing::error!("the Vue compiler is not available to check a Vue SFC canvas: {error}");
                None
            }
        };
        prepared(previous, input, compiler.as_deref()).map_err(|problems| Refused {
            error: error_json(&problems, input.patch.is_some() && input.program.is_none()),
            problems: problems.iter().map(super::openui::problem_json).collect(),
        })
    }
}

crate::submit_dialect!(VueSfc);

// ---- the frame document (GET /assist/sandbox) ----

/// The frame document and its Content-Security-Policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameDocument {
    pub html: String,
    pub csp: String,
}

/// The headers the frame document is sent with besides its CSP: no referrer, no sniffing, no powerful feature.
pub const FRAME_HEADERS: &[(&str, &str)] = &[
    ("x-content-type-options", "nosniff"),
    ("referrer-policy", "no-referrer"),
    ("cache-control", "no-cache"),
    ("cross-origin-resource-policy", "same-origin"),
    (
        "permissions-policy",
        "camera=(), microphone=(), geolocation=(), payment=(), usb=(), serial=(), hid=(), bluetooth=(), display-capture=(), \
         clipboard-read=(), clipboard-write=(), fullscreen=(), publickey-credentials-get=()",
    ),
];

/// Whether `origin` is a plain `http(s)://host[:port]`, safe to write into a policy.
pub fn plain_origin(origin: &str) -> bool {
    let Some(rest) = origin.strip_prefix("https://").or_else(|| origin.strip_prefix("http://")) else {
        return false;
    };
    !rest.is_empty()
        && rest.len() <= 255
        && rest.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | ':' | '[' | ']'))
}

/// The frame document of the Vue SFC sandbox for the page at `origin` (`https://host:port`, which the policy writes
/// out because `'self'` does not match the frame's opaque origin), cache-busted by `version`; `external` (the user's
/// choice) lets images come from other sites as well. The policy has the `sandbox` directive, so the document is
/// sandboxed even opened on its own; no network (`connect-src 'none'`), no form, no frame, no worker, no plugin; scripts
/// only Vue, Quasar and the frame runtime (`'unsafe-eval'` because the component is compiled in the frame); styles and
/// fonts only the vendored ones; images `data:` and the site's static images; framed only by the site.
pub fn frame_document(origin: &str, version: &str, external: bool) -> FrameDocument {
    let version: String = version.chars().filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_')).take(40).collect();
    let query = if version.is_empty() { String::new() } else { format!("?v={version}") };
    let images = if external { " https:" } else { "" };
    let csp = format!(
        "sandbox allow-scripts; default-src 'none'; \
         script-src {origin}/libs/vue/ {origin}/libs/quasar/ {origin}/js/WebrootAssistSandbox.js 'unsafe-eval'; \
         style-src {origin}/libs/ 'unsafe-inline'; font-src {origin}/libs/; img-src data: {origin}/images/ {origin}/libs/{images}; \
         connect-src 'none'; media-src 'none'; object-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; \
         manifest-src 'none'; form-action 'none'; frame-ancestors 'self' {origin}; base-uri 'none'"
    );
    let html = format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Canvas</title>\
         <link rel=\"stylesheet\" href=\"/libs/quasar/quasar.prod.css{query}\">\
         <link rel=\"stylesheet\" href=\"/libs/material-symbols/outlined.css{query}\">\
         <link rel=\"stylesheet\" href=\"/libs/material-icons/material-icons.css{query}\">\
         <style>html,body{{margin:0;padding:0;background:transparent;overflow:hidden}}body.body--dark{{color:#fff}}\
         a{{color:#1976d2}}body.body--dark a{{color:#90caf9}}</style>\
         </head><body><div id=\"m-sfc-app\"></div>\
         <script src=\"/libs/vue/vue.global.prod.js{query}\"></script>\
         <script src=\"/libs/quasar/quasar.umd.prod.js{query}\"></script>\
         <script src=\"/js/WebrootAssistSandbox.js{query}\"></script>\
         </body></html>\n"
    );
    FrameDocument { html, csp }
}

#[cfg(test)]
#[path = "vue_sfc_tests.rs"]
mod tests;
