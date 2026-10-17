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

//! `moqui convert --groovy`: the mechanical part of the Groovy migration of a component tree such as mantle.
//!
//! Only rewrites whose meaning is the same in JavaScript are made, and only in the expression places of XML (the
//! expression attributes, `${}` in any other attribute or text, and `<script>` and `<expression>` bodies):
//!
//! - a Groovy map literal `[a: 1, (k): v]` becomes `{a: 1, [k]: v}`, and `[:]` becomes `{}`;
//! - `context + [a: 1]` becomes `Object.assign({}, context, {a: 1})`;
//! - Elvis `a ?: b` becomes `a || b`, except before a ternary at the same depth, where `||` would bind differently;
//! - in a `condition` attribute, a name that is the `list` of an `entity-find` or `entity-find-related` in the same file
//!   becomes `(name?.length > 0)`, and `!name` becomes `!name?.length`: an empty list is false in Groovy and an object
//!   (always true) in JavaScript. `?.length` holds for an entity list, an array, and null;
//! - `'lit'.equals(x)` and `x.equals('lit')` (a name chain) become `('lit' === x)` and `(x === 'lit')`;
//! - `x as String` becomes `x?.toString()` (of any postfix expression); `x as BigDecimal` and `x as int` (or `Integer`,
//!   `long`, `Long`) of a name chain, an index, or a parenthesized group with no call in it become `(x == null ? null : toDecimal(x))` and `(x == null ? null : Math.trunc(toDecimal(x).toNumber()))`,
//!   as a Groovy cast of null is null; a numeric cast of anything else is left for review;
//! - `x in [..]` becomes `[..].includes(x)` where `x` is one postfix expression that starts at a boundary (so
//!   `!(x in [..])` and `a && x in [..]` work, and `!x in [..]`, `a + x in [..]`, and `for (x in [..])` are left);
//! - `.contains(` becomes `.includes(` on a string literal or a name that is a found list of the file;
//! - `new ArrayList()` becomes `[]`, and `new HashMap()` or `new LinkedHashMap()` becomes `{}` (generics or not);
//! - an EDI grammar file (a `.groovy` file that is one list literal) gets a `.js` copy in JavaScript, and an XML
//!   location that names the `.groovy` file names the `.js` one.
//!
//! Everything else is left for the AI pass the pre-scan feeds; `moqui lint --groovy` reports it.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::context::Error;

/// What a rewrite changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RewriteKind {
    MapLiteral,
    ContextPlus,
    Elvis,
    EmptyList,
    StringEquals,
    Cast,
    InList,
    ListIncludes,
    EmptyCollection,
    QuotedProperty,
    GString,
    GrammarFile,
    GrammarLocation,
}

impl RewriteKind {
    pub fn name(self) -> &'static str {
        match self {
            RewriteKind::MapLiteral => "map-literal",
            RewriteKind::ContextPlus => "context-plus",
            RewriteKind::Elvis => "elvis",
            RewriteKind::EmptyList => "empty-list",
            RewriteKind::StringEquals => "string-equals",
            RewriteKind::Cast => "cast",
            RewriteKind::InList => "in-list",
            RewriteKind::ListIncludes => "list-includes",
            RewriteKind::EmptyCollection => "empty-collection",
            RewriteKind::QuotedProperty => "quoted-property",
            RewriteKind::GString => "gstring",
            RewriteKind::GrammarFile => "grammar-file",
            RewriteKind::GrammarLocation => "grammar-location",
        }
    }
}

/// One changed expression place: the line of the file it starts on, what changed in it, and its text before and after.
#[derive(Clone, Debug)]
pub struct Rewrite {
    pub line: u32,
    pub kinds: Vec<RewriteKind>,
    pub before: String,
    pub after: String,
}

/// A place the converter saw and did not rewrite, with the reason.
#[derive(Clone, Debug)]
pub struct Left {
    pub line: u32,
    pub reason: &'static str,
    pub text: String,
}

/// The conversion of one file.
#[derive(Clone, Debug, Default)]
pub struct FileConversion {
    pub path: PathBuf,
    /// For a grammar file, the `.js` file written beside it.
    pub written: Option<PathBuf>,
    pub text: String,
    pub rewrites: Vec<Rewrite>,
    pub left: Vec<Left>,
    /// The found-list names of the file, which the list rewrites read.
    pub lists: BTreeSet<String>,
}

/// The conversion of a tree.
#[derive(Clone, Debug, Default)]
pub struct ConvertReport {
    pub files: Vec<FileConversion>,
    /// Grammar files that did not become JavaScript, with the parse error.
    pub failed_grammars: Vec<(PathBuf, String)>,
}

impl ConvertReport {
    pub fn count(&self, kind: RewriteKind) -> usize {
        self.files
            .iter()
            .flat_map(|file| &file.rewrites)
            .filter(|rewrite| rewrite.kinds.contains(&kind))
            .count()
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "Groovy conversion");
        let _ = writeln!(out, "files changed: {}", self.files.len());
        for kind in [
            RewriteKind::MapLiteral,
            RewriteKind::ContextPlus,
            RewriteKind::Elvis,
            RewriteKind::EmptyList,
            RewriteKind::StringEquals,
            RewriteKind::Cast,
            RewriteKind::InList,
            RewriteKind::ListIncludes,
            RewriteKind::EmptyCollection,
            RewriteKind::QuotedProperty,
            RewriteKind::GString,
            RewriteKind::GrammarFile,
            RewriteKind::GrammarLocation,
        ] {
            let _ = writeln!(out, "  {:<20} {:>5}", kind.name(), self.count(kind));
        }
        let left: usize = self.files.iter().map(|file| file.left.len()).sum();
        let _ = writeln!(out, "places left for review: {left}");
        for (path, error) in &self.failed_grammars {
            let _ = writeln!(out, "grammar not converted: {}: {error}", path.display());
        }
        for file in &self.files {
            match &file.written {
                Some(written) => {
                    let _ = writeln!(out, "\n{} -> {}", file.path.display(), written.display());
                }
                None => {
                    let _ = writeln!(out, "\n{}", file.path.display());
                }
            }
            for rewrite in &file.rewrites {
                let kinds: Vec<&str> = rewrite.kinds.iter().map(|kind| kind.name()).collect();
                let _ = writeln!(out, "  {}: [{}]", rewrite.line, kinds.join(","));
                let _ = writeln!(out, "    - {}", one_line(&rewrite.before));
                let _ = writeln!(out, "    + {}", one_line(&rewrite.after));
            }
            for left in &file.left {
                let _ = writeln!(out, "  {}: left, {}: {}", left.line, left.reason, one_line(&left.text));
            }
        }
        out
    }
}

/// Convert every XML file and grammar file under `roots`. With `write` the changed files are written in place and each
/// grammar's `.js` file is written beside it; without it nothing is written.
///
/// # Errors
///
/// Returns an error when a directory or file cannot be read or written.
pub fn convert_tree(roots: &[PathBuf], write: bool) -> Result<ConvertReport, Error> {
    let mut xml = Vec::new();
    let mut groovy = Vec::new();
    for root in roots {
        super::screen_lint::collect_all(root, &mut xml, &mut groovy)?;
    }
    xml.sort();
    groovy.sort();
    let mut report = ConvertReport::default();
    let mut grammars = BTreeSet::new();
    for path in groovy {
        let text = read(&path)?;
        if !is_grammar(&text) {
            continue;
        }
        let converted = convert_literals(&text);
        if let Err(err) = super::parse_js(&converted) {
            report.failed_grammars.push((path, err.to_string()));
            continue;
        }
        let target = path.with_extension("js");
        if write {
            std::fs::write(&target, &converted).map_err(|err| Error::new(format!("{}: {err}", target.display())))?;
        }
        if let Some(stem) = path.file_stem() {
            grammars.insert(stem.to_string_lossy().to_string());
        }
        report.files.push(FileConversion {
            path,
            written: Some(target),
            rewrites: vec![Rewrite {
                line: 1,
                kinds: vec![RewriteKind::GrammarFile],
                before: first_line(&text),
                after: first_line(&converted),
            }],
            text: converted,
            left: Vec::new(),
            lists: BTreeSet::new(),
        });
    }
    for path in xml {
        let text = read(&path)?;
        let mut file = convert_xml(&text);
        let located = rename_grammar_locations(&file.text, &grammars, &mut file.rewrites);
        file.text = located;
        if file.rewrites.is_empty() && file.left.is_empty() {
            continue;
        }
        if write && !file.rewrites.is_empty() {
            std::fs::write(&path, &file.text).map_err(|err| Error::new(format!("{}: {err}", path.display())))?;
        }
        file.path = path;
        report.files.push(file);
    }
    Ok(report)
}

fn read(path: &Path) -> Result<String, Error> {
    std::fs::read_to_string(path).map_err(|err| Error::new(format!("{}: {err}", path.display())))
}

fn first_line(text: &str) -> String {
    text.lines()
        .find(|line| !line.trim().is_empty() && !line.trim_start().starts_with("//"))
        .unwrap_or("")
        .to_string()
}

/// A grammar file is one Groovy list literal, after line comments.
fn is_grammar(text: &str) -> bool {
    let body: String = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let body = body.trim();
    body.starts_with('[') && body.ends_with(']')
}

/// `component://.../X12Envelope.groovy` becomes `.js` for each grammar that was converted.
fn rename_grammar_locations(text: &str, grammars: &BTreeSet<String>, rewrites: &mut Vec<Rewrite>) -> String {
    let mut out = text.to_string();
    for stem in grammars {
        let from = format!("/{stem}.groovy\"");
        let to = format!("/{stem}.js\"");
        let mut search = 0;
        while let Some(found) = out[search..].find(&from) {
            let at = search + found;
            let line = line_of(&out, at);
            let start = out[..at].rfind('"').map_or(at, |quote| quote + 1);
            let before = out[start..at + from.len() - 1].to_string();
            out.replace_range(at..at + from.len(), &to);
            let after = out[start..at + to.len() - 1].to_string();
            rewrites.push(Rewrite {
                line,
                kinds: vec![RewriteKind::GrammarLocation],
                before,
                after,
            });
            search = at + to.len();
        }
    }
    rewrites.sort_by_key(|rewrite| rewrite.line);
    out
}

/// Expression attributes: the whole value is an expression.
fn is_expression_attr(element: &str, attr: &str) -> bool {
    match attr {
        // `default` is an expression on a service parameter and an entity field.
        "condition" | "from" | "in-map" | "parameter-map" | "text-map" | "default" => {
            attr != "default" || matches!(element, "parameter" | "field")
        }
        "list" => matches!(
            element,
            "section-iterate"
                | "iterate"
                | "form-list"
                | "tree-sub-node"
                | "list-options"
                | "order-map-list"
                | "filter-map-list"
        ),
        "map" => matches!(element, "form-single" | "iterate"),
        _ => false,
    }
}

/// Convert the expression places of one XML file. The rest of the text is copied as it is.
pub fn convert_xml(text: &str) -> FileConversion {
    let mut file = FileConversion {
        lists: entity_find_lists(text),
        ..FileConversion::default()
    };
    let lists = file.lists.clone();
    let mut out = String::with_capacity(text.len());
    let mut stack: Vec<String> = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if text[index..].starts_with("<!--") {
            let end = text[index..].find("-->").map_or(text.len(), |end| index + end + 3);
            out.push_str(&text[index..end]);
            index = end;
        } else if text[index..].starts_with("<![CDATA[") {
            let start = index + "<![CDATA[".len();
            let end = text[start..].find("]]>").map_or(text.len(), |end| start + end);
            out.push_str("<![CDATA[");
            let in_script = stack
                .last()
                .is_some_and(|name| name == "script" || name == "expression");
            out.push_str(&convert_place(&mut file, text, start, &text[start..end], in_script));
            out.push_str("]]>");
            index = (end + 3).min(text.len());
        } else if text[index..].starts_with("<?") || text[index..].starts_with("<!") {
            let end = text[index..].find('>').map_or(text.len(), |end| index + end + 1);
            out.push_str(&text[index..end]);
            index = end;
        } else if text[index..].starts_with("</") {
            let end = text[index..].find('>').map_or(text.len(), |end| index + end + 1);
            stack.pop();
            out.push_str(&text[index..end]);
            index = end;
        } else if bytes[index] == b'<' {
            index = convert_tag(&mut file, text, index, &mut out, &mut stack, &lists);
        } else {
            let end = text[index..].find('<').map_or(text.len(), |end| index + end);
            let in_script = stack
                .last()
                .is_some_and(|name| name == "script" || name == "expression");
            out.push_str(&convert_place(&mut file, text, index, &text[index..end], in_script));
            index = end;
        }
    }
    file.text = out;
    file
}

/// Copy one start tag, converting the expression places of its attributes. Returns the index after the tag.
fn convert_tag(
    file: &mut FileConversion,
    text: &str,
    start: usize,
    out: &mut String,
    stack: &mut Vec<String>,
    lists: &BTreeSet<String>,
) -> usize {
    let bytes = text.as_bytes();
    let mut index = start + 1;
    while index < bytes.len() && !bytes[index].is_ascii_whitespace() && bytes[index] != b'>' && bytes[index] != b'/' {
        index += 1;
    }
    let element = text[start + 1..index].to_string();
    out.push_str(&text[start..index]);
    loop {
        let rest = index;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            out.push_str(&text[rest..]);
            return bytes.len();
        }
        if bytes[index] == b'>' || text[index..].starts_with("/>") {
            let self_closing = bytes[index] == b'/';
            let end = if self_closing { index + 2 } else { index + 1 };
            out.push_str(&text[rest..end]);
            if !self_closing {
                stack.push(element);
            }
            return end;
        }
        let name_start = index;
        while index < bytes.len() && bytes[index] != b'=' && !bytes[index].is_ascii_whitespace() && bytes[index] != b'>'
        {
            index += 1;
        }
        let attr = text[name_start..index].to_string();
        while index < bytes.len() && (bytes[index].is_ascii_whitespace() || bytes[index] == b'=') {
            index += 1;
        }
        if index >= bytes.len() || (bytes[index] != b'"' && bytes[index] != b'\'') {
            out.push_str(&text[rest..index]);
            continue;
        }
        let quote = bytes[index];
        let value_start = index + 1;
        let value_end = text[value_start..]
            .find(quote as char)
            .map_or(text.len(), |end| value_start + end);
        out.push_str(&text[rest..value_start]);
        let value = &text[value_start..value_end];
        if attr == "condition" && is_expression_attr(&element, &attr) {
            let converted = convert_place(file, text, value_start, value, true);
            let (checked, count) = empty_list(&converted, lists);
            if count > 0 {
                file.rewrites.push(Rewrite {
                    line: line_of(text, value_start),
                    kinds: vec![RewriteKind::EmptyList],
                    before: converted,
                    after: checked.clone(),
                });
            }
            out.push_str(&checked);
        } else if is_expression_attr(&element, &attr) {
            out.push_str(&convert_place(file, text, value_start, value, true));
        } else {
            out.push_str(&convert_place(file, text, value_start, value, false));
        }
        index = value_end;
        if index < bytes.len() {
            out.push(quote as char);
            index += 1;
        }
    }
}

/// Convert one place: the whole text when it is an expression or a script, else each `${}` in it.
fn convert_place(file: &mut FileConversion, whole: &str, offset: usize, text: &str, expression: bool) -> String {
    if expression {
        return convert_expression_at(file, whole, offset, text);
    }
    if !text.contains("${") {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (start, end) in dollar_spans(text) {
        out.push_str(&text[last..start]);
        out.push_str(&convert_expression_at(file, whole, offset + start, &text[start..end]));
        last = end;
    }
    out.push_str(&text[last..]);
    out
}

fn convert_expression_at(file: &mut FileConversion, whole: &str, offset: usize, text: &str) -> String {
    let line = line_of(whole, offset);
    let (converted, kinds, left) = rewrite_expression_with(text, &file.lists);
    for reason in left {
        file.left.push(Left {
            line,
            reason,
            text: text.to_string(),
        });
    }
    if !kinds.is_empty() {
        file.rewrites.push(Rewrite {
            line,
            kinds,
            before: text.to_string(),
            after: converted.clone(),
        });
    }
    converted
}

/// Byte ranges of the string literals of `text` (single, double, and triple quotes), so rewrites skip them.
fn string_spans(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\'' || byte == b'"' {
            let triple = text[index..].starts_with(if byte == b'"' { "\"\"\"" } else { "\'\'\'" });
            let (open, close) = if triple {
                (3, if byte == b'"' { "\"\"\"" } else { "\'\'\'" })
            } else {
                (1, if byte == b'"' { "\"" } else { "'" })
            };
            let mut end = index + open;
            loop {
                match text[end..].find(close) {
                    Some(at) if !triple && at > 0 && bytes[end + at - 1] == b'\\' => end += at + 1,
                    Some(at) => {
                        end += at + close.len();
                        break;
                    }
                    None => {
                        end = text.len();
                        break;
                    }
                }
            }
            spans.push((index, end));
            index = end;
        } else {
            index += 1;
        }
    }
    spans
}

fn in_spans(spans: &[(usize, usize)], at: usize) -> bool {
    spans.iter().any(|(start, end)| at >= *start && at < *end)
}

/// The start of the name chain (`a.b?.c`) that ends at `end`, if the text before `end` is one.
fn chain_start(text: &str, end: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut start = end;
    while start > 0 {
        let byte = bytes[start - 1];
        if is_ident(byte) || byte == b'.' || (byte == b'?' && start < end && bytes[start] == b'.') {
            start -= 1;
        } else {
            break;
        }
    }
    let chain = &text[start..end];
    let first = chain.as_bytes().first().copied()?;
    if !(first.is_ascii_alphabetic() || first == b'_' || first == b'$') || chain.ends_with('.') || chain.ends_with('?')
    {
        return None;
    }
    // A chain right after a `.`, `)` or `]` is the end of a longer expression; leave it.
    if start > 0 && matches!(bytes[start - 1], b'.' | b')' | b']') {
        return None;
    }
    Some(start)
}

/// `'lit'.equals(x)` to `('lit' === x)`, and `chain.equals('lit')` to `(chain === 'lit')`.
fn string_equals(text: &str) -> (String, usize) {
    let mut out = text.to_string();
    let mut count = 0;
    loop {
        let spans = string_spans(&out);
        let mut done = true;
        for (start, end) in &spans {
            let literal = &out[*start..*end];
            if literal.contains("${") || literal.starts_with("\"\"\"") || literal.starts_with("\'\'\'") {
                continue;
            }
            // 'lit'.equals(arg)
            if out[*end..].starts_with(".equals(") {
                let open = end + ".equals".len();
                if let Some(close) = matching(&out, open, b'(', b')') {
                    let arg = out[open + 1..close].trim().to_string();
                    let compared = parenthesize(&out, *start, close + 1, &format!("{literal} === {arg}"));
                    out = format!("{}{compared}{}", &out[..*start], &out[close + 1..]);
                    count += 1;
                    done = false;
                    break;
                }
            }
            // chain.equals('lit')
            let before = &out[..*start];
            if before.ends_with(".equals(") && out[*end..].trim_start().starts_with(')') {
                let call = start - ".equals(".len();
                if let Some(chain) = chain_start(&out, call) {
                    let close = end + out[*end..].find(')').unwrap_or(0);
                    let compared =
                        parenthesize(&out, chain, close + 1, &format!("{} === {literal}", &out[chain..call]));
                    out = format!("{}{compared}{}", &out[..chain], &out[close + 1..]);
                    count += 1;
                    done = false;
                    break;
                }
            }
        }
        if done {
            return (out, count);
        }
    }
}

/// `value.'Rel#entity'` and `value?.'name'`: a Groovy property named by a string is a bracket access in JavaScript,
/// `value['Rel#entity']` and `value?.['name']`.
fn quoted_properties(text: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut count = 0;
    let mut last = 0;
    for (start, end) in string_spans(text) {
        let literal = &text[start..end];
        if literal.contains("${") || literal.len() < 2 || literal.starts_with("\"\"\"") || literal.starts_with("'''") {
            continue;
        }
        let before = &text[last..start];
        let (dot, safe) = if before.ends_with("?.") {
            (start - 2, true)
        } else if before.ends_with('.') {
            (start - 1, false)
        } else {
            continue;
        };
        let owner = text[..dot].chars().next_back();
        if !owner.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ')' || c == ']') {
            continue;
        }
        out.push_str(&text[last..dot]);
        out.push_str(if safe { "?.[" } else { "[" });
        out.push_str(literal);
        out.push(']');
        last = end;
        count += 1;
    }
    out.push_str(&text[last..]);
    (out, count)
}

/// `"a ${b} c"`: a double-quoted Groovy string with `${...}` interpolates, so it is a JavaScript template literal,
/// `` `a ${b} c` ``. Triple double quotes are the same. One with a bare `$name`, a double quote inside an
/// interpolation, or a backtick is left.
fn gstrings(text: &str) -> (String, usize, usize) {
    let mut out = String::with_capacity(text.len());
    let (mut count, mut skipped) = (0, 0);
    let mut last = 0;
    for (start, end) in string_spans(text) {
        let literal = &text[start..end];
        let quote = if literal.starts_with("\"\"\"") && literal.len() >= 6 && literal.ends_with("\"\"\"") {
            3
        } else if literal.starts_with('"') && literal.len() >= 2 && literal.ends_with('"') {
            1
        } else {
            continue;
        };
        let body = &literal[quote..literal.len() - quote];
        if !body.contains('$') {
            continue;
        }
        let bytes = body.as_bytes();
        let bare_name = (0..bytes.len()).any(|at| {
            bytes[at] == b'$'
                && (at == 0 || bytes[at - 1] != b'\\')
                && bytes
                    .get(at + 1)
                    .is_some_and(|next| next.is_ascii_alphabetic() || *next == b'_')
        });
        // a double quote inside an interpolation ends the span early, so the span is not the whole literal
        let nested_quote = quote == 1 && text[end..].starts_with(|c: char| c.is_alphanumeric());
        if bare_name || body.contains('`') || nested_quote {
            skipped += 1;
            continue;
        }
        if !body.contains("${") {
            continue;
        }
        out.push_str(&text[last..start]);
        out.push('`');
        out.push_str(body);
        out.push('`');
        last = end;
        count += 1;
    }
    out.push_str(&text[last..]);
    (out, count, skipped)
}

/// `inner` in parentheses, unless the text it replaces (`start..end` of `text`) is already the whole of a parenthesized
/// group, such as an `if (...)` condition.
fn parenthesize(text: &str, start: usize, end: usize, inner: &str) -> String {
    let wrapped = text[..start].trim_end().ends_with('(') && text[end..].trim_start().starts_with(')');
    if wrapped {
        inner.to_string()
    } else {
        format!("({inner})")
    }
}

/// `x as String`, and the numeric casts of a name chain. See the module notes.
fn casts(text: &str) -> (String, usize, usize) {
    let mut out = text.to_string();
    let mut count = 0;
    let mut skipped = 0;
    let mut from = 0;
    loop {
        let spans = string_spans(&out);
        let Some(found) = find_cast(&out, from, &spans) else {
            return (out, count, skipped);
        };
        let (as_at, type_end, kind) = found;
        let before = out[..as_at].trim_end();
        let operand_end = before.len();
        let Some(operand_start) = postfix_start(&out, operand_end).filter(|start| !in_spans(&spans, *start)) else {
            from = type_end;
            continue;
        };
        // `as` binds looser than arithmetic and ordering in Groovy (`a + b as String` casts the sum), so the operand must
        // start where an expression at that level starts.
        if !starts_cast_operand(out[..operand_start].trim_end()) {
            skipped += 1;
            from = type_end;
            continue;
        }
        let operand = out[operand_start..operand_end].to_string();
        let simple = pure_operand(&operand);
        let replacement = match kind {
            "String" => {
                let wrapped = operand.starts_with('(') && matching(&operand, 0, b'(', b')') == Some(operand.len() - 1);
                // The operand is one postfix expression, so `?.toString()` applies to all of it.
                if wrapped || postfix_start(&operand, operand.len()) == Some(0) {
                    format!("{operand}?.toString()")
                } else {
                    format!("({operand})?.toString()")
                }
            }
            _ if !simple => {
                skipped += 1;
                from = type_end;
                continue;
            }
            "BigDecimal" => parenthesize(
                &out,
                operand_start,
                type_end,
                &format!("{operand} == null ? null : toDecimal({operand})"),
            ),
            _ => parenthesize(
                &out,
                operand_start,
                type_end,
                &format!("{operand} == null ? null : Math.trunc(toDecimal({operand}).toNumber())"),
            ),
        };
        out = format!("{}{replacement}{}", &out[..operand_start], &out[type_end..]);
        from = operand_start + replacement.len();
        count += 1;
    }
}

/// True when text ending with `before` is followed by the start of an operand that a Groovy `as` covers whole: nothing,
/// an opening bracket, a separator, an assignment or equality, a logical operator, or `return`.
fn starts_cast_operand(before: &str) -> bool {
    if before.is_empty() {
        return true;
    }
    // `&lt;`, `&gt;` and their `=` forms are ordering operators in XML text; `&amp;&amp;` still ends in `;` as `&&` does.
    if before.ends_with("<=")
        || before.ends_with(">=")
        || before.ends_with("&lt;")
        || before.ends_with("&gt;")
        || before.ends_with("&lt;=")
        || before.ends_with("&gt;=")
    {
        return false;
    }
    ["(", ",", "[", "{", "?", ":", "=", "&&", "||", ";", "return"]
        .iter()
        .any(|end| before.ends_with(end))
}

/// The next ` as TYPE` outside a string from `from`: where `as` starts, where the type ends, and the type.
fn find_cast(text: &str, from: usize, spans: &[(usize, usize)]) -> Option<(usize, usize, &'static str)> {
    let mut search = from;
    while let Some(at) = text[search..].find(" as ") {
        let as_at = search + at + 1;
        search = as_at + 2;
        if in_spans(spans, as_at) {
            continue;
        }
        let rest = &text[as_at + 3..];
        let trimmed = rest.trim_start();
        let type_start = as_at + 3 + (rest.len() - trimmed.len());
        for (name, kind) in [
            ("BigDecimal", "BigDecimal"),
            ("String", "String"),
            ("Integer", "int"),
            ("int", "int"),
            ("Long", "int"),
            ("long", "int"),
        ] {
            if trimmed.starts_with(name) {
                let type_end = type_start + name.len();
                let next = text.as_bytes().get(type_end).copied();
                if next.is_some_and(|byte| is_ident(byte) || byte == b'[' || byte == b'.') {
                    continue;
                }
                return Some((as_at, type_end, kind));
            }
        }
    }
    None
}

/// The start of the postfix expression (`a.b(c)[0]?.d`, a parenthesized group, a name) that ends at `end`.
fn postfix_start(text: &str, end: usize) -> Option<usize> {
    let mut pos = end;
    loop {
        pos = element_start(text, pos)?;
        let before = &text[..pos];
        if before.ends_with("?.") {
            pos -= 2;
        } else if before.ends_with('.') {
            pos -= 1;
        } else {
            return Some(pos);
        }
    }
}

/// The start of one element ending at `pos`: a name, optionally followed by call and index suffixes, or a bare group.
fn element_start(text: &str, pos: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut at = pos;
    loop {
        let last = *bytes.get(at.checked_sub(1)?)?;
        if last == b')' {
            at = matching_back(text, at - 1, b'(', b')')?;
            if at > 0 && is_ident(bytes[at - 1]) {
                return name_start(text, at);
            }
            return Some(at);
        } else if last == b']' {
            at = matching_back(text, at - 1, b'[', b']')?;
            let prev = *bytes.get(at.checked_sub(1)?)?;
            if !(is_ident(prev) || prev == b')' || prev == b']') {
                return None;
            }
        } else if is_ident(last) {
            return name_start(text, at);
        } else {
            return None;
        }
    }
}

fn name_start(text: &str, end: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut start = end;
    while start > 0 && is_ident(bytes[start - 1]) {
        start -= 1;
    }
    let first = *bytes.get(start)?;
    (first.is_ascii_alphabetic() || first == b'_' || first == b'$').then_some(start)
}

/// True when evaluating the operand twice is the same as once: names, dots, indexes, grouping, and arithmetic or logic
/// on them, with no call, assignment, string, or ternary.
fn pure_operand(operand: &str) -> bool {
    let bytes = operand.as_bytes();
    if operand.contains("//") || operand.contains("/*") || operand.contains("++") || operand.contains("--") {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        let allowed = is_ident(*byte)
            || matches!(
                *byte,
                b'.' | b'?' | b' ' | b'|' | b'&' | b'+' | b'-' | b'*' | b'/' | b'%' | b'(' | b')' | b'[' | b']'
            );
        if !allowed {
            return false;
        }
        if *byte == b'(' || *byte == b'[' {
            let prev = operand[..index].trim_end().bytes().last();
            if *byte == b'(' && prev.is_some_and(|p| is_ident(p) || p == b')' || p == b']') {
                return false;
            }
        }
        if *byte == b'?' && !(bytes.get(index + 1) == Some(&b'.') || index > 0 && bytes[index - 1] == b'?') {
            return false;
        }
    }
    !operand.trim().is_empty()
}

/// `x in [..]` to `[..].includes(x)`. See the module notes.
fn in_lists(text: &str) -> (String, usize) {
    let mut out = text.to_string();
    let mut count = 0;
    let mut from = 0;
    loop {
        let spans = string_spans(&out);
        let Some((at, open, close)) = find_in(&out, from, &spans) else {
            return (out, count);
        };
        from = at + 2;
        let after = &out[close + 1..];
        let rest = after.trim_start();
        let gap = &after[..after.len() - rest.len()];
        let follows = rest.is_empty()
            || [")", ",", "]", "}", ";", "&&", "||", "&amp;&amp;", ":"]
                .iter()
                .any(|s| rest.starts_with(s))
            || (rest.starts_with('?') && !rest.starts_with("?."))
            || (gap.contains('\n') && rest.starts_with(|c: char| c.is_ascii_alphabetic() || c == '}'));
        if !follows {
            continue;
        }
        let end = out[..at].trim_end().len();
        let Some(start) = postfix_start(&out, end).filter(|start| !in_spans(&spans, *start)) else {
            continue;
        };
        let before = out[..start].trim_end();
        // `for (x in [..])` and `for (String x in [..])` are loops, not tests.
        let last_word: String = before
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        if (!last_word.is_empty() && last_word != "return")
            || before.trim_end_matches('(').trim_end().ends_with("for") && before.ends_with('(')
            || !starts_cast_operand(before)
        {
            continue;
        }
        let replacement = format!("{}.includes({})", &out[open..=close], &out[start..end]);
        out = format!("{}{replacement}{}", &out[..start], &out[close + 1..]);
        from = start;
        count += 1;
    }
}

/// The next `x in [..]` operator outside a string or `//` comment from `from`: where `in` starts, and the list's
/// brackets. A map literal is not a list.
fn find_in(text: &str, from: usize, spans: &[(usize, usize)]) -> Option<(usize, usize, usize)> {
    let bytes = text.as_bytes();
    let mut search = from;
    while let Some(found) = text[search..].find("in") {
        let at = search + found;
        search = at + 2;
        if at == 0 || !(bytes[at - 1].is_ascii_whitespace() || bytes[at - 1] == b')') || in_spans(spans, at) {
            continue;
        }
        if !bytes.get(at + 2).is_some_and(|b| b.is_ascii_whitespace() || *b == b'[') {
            continue;
        }
        let line_start = text[..at].rfind('\n').map_or(0, |n| n + 1);
        if text[line_start..at].contains("//") {
            continue;
        }
        let rest = text[at + 2..].trim_start();
        if !rest.starts_with('[') {
            continue;
        }
        let open = text.len() - rest.len();
        let Some(close) = matching(text, open, b'[', b']') else {
            continue;
        };
        if is_map(&text[open + 1..close]) {
            continue;
        }
        return Some((at, open, close));
    }
    None
}

/// The opening bracket that matches the closing one at `close`, scanning back.
fn matching_back(text: &str, close: usize, opening: u8, closing: u8) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut index = close + 1;
    while index > 0 {
        index -= 1;
        if bytes[index] == closing {
            depth += 1;
        } else if bytes[index] == opening {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

/// `.contains(` to `.includes(` on a string literal or a found list of the file.
fn list_includes(text: &str, lists: &BTreeSet<String>) -> (String, usize) {
    let spans = string_spans(text);
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut count = 0;
    let mut search = 0;
    while let Some(at) = text[search..].find(".contains(") {
        let at = search + at;
        search = at + 1;
        if in_spans(&spans, at) {
            continue;
        }
        let literal = spans.iter().any(|(_, end)| *end == at);
        let named = chain_start(text, at).is_some_and(|start| lists.contains(&text[start..at]));
        if literal || named {
            out.push_str(&text[last..at]);
            out.push_str(".includes(");
            last = at + ".contains(".len();
            count += 1;
        }
    }
    out.push_str(&text[last..]);
    (out, count)
}

/// `new ArrayList()` to `[]`, `new HashMap()` and `new LinkedHashMap()` to `{}`, with or without generics.
fn empty_collections(text: &str) -> (String, usize) {
    let spans = string_spans(text);
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut count = 0;
    let mut search = 0;
    while let Some(at) = text[search..].find("new ") {
        let at = search + at;
        search = at + 4;
        if in_spans(&spans, at) || (at > 0 && is_ident(text.as_bytes()[at - 1])) {
            continue;
        }
        let rest = &text[at + 4..];
        let (name, empty) = if rest.starts_with("ArrayList") {
            ("ArrayList", "[]")
        } else if rest.starts_with("LinkedHashMap") {
            ("LinkedHashMap", "{}")
        } else if rest.starts_with("HashMap") {
            ("HashMap", "{}")
        } else {
            continue;
        };
        let mut index = at + 4 + name.len();
        if text[index..].starts_with('<') {
            match matching(text, index, b'<', b'>') {
                Some(close) => index = close + 1,
                None => continue,
            }
        }
        if !text[index..].starts_with("()") {
            continue;
        }
        out.push_str(&text[last..at]);
        out.push_str(empty);
        last = index + 2;
        search = last;
        count += 1;
    }
    out.push_str(&text[last..]);
    (out, count)
}

/// The `list` names of the `entity-find` and `entity-find-related` elements of a file: names that always hold a list.
fn entity_find_lists(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for tag in ["<entity-find ", "<entity-find-related "] {
        let mut rest = text;
        while let Some(at) = rest.find(tag) {
            let after = &rest[at + tag.len()..];
            let end = after.find('>').unwrap_or(after.len());
            let attrs = &after[..end];
            if let Some(start) = attrs
                .find(" list=\"")
                .map(|at| at + 7)
                .or_else(|| attrs.strip_prefix("list=\"").map(|_| 6))
            {
                if let Some(len) = attrs[start..].find('"') {
                    let name = &attrs[start..start + len];
                    if !name.is_empty() && name.bytes().all(is_ident) {
                        names.insert(name.to_string());
                    }
                }
            }
            rest = &after[end..];
        }
    }
    names
}

/// `name` to `(name?.length > 0)` and `!name` to `!name?.length` for each standalone name of `lists` in a condition.
/// Strings are skipped, and a name used as an object (`name.size()`, `name[0]`, `a.name`) is left alone.
fn empty_list(text: &str, lists: &BTreeSet<String>) -> (String, usize) {
    if lists.is_empty() {
        return (text.to_string(), 0);
    }
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len() + 16);
    let mut count = 0;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\'' || byte == b'"' {
            let end = text[index + 1..]
                .find(byte as char)
                .map_or(text.len(), |end| index + 1 + end + 1);
            out.push_str(&text[index..end]);
            index = end;
            continue;
        }
        if !is_ident(byte) || (index > 0 && is_ident(bytes[index - 1])) {
            out.push(byte as char);
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && is_ident(bytes[index]) {
            index += 1;
        }
        let name = &text[start..index];
        let before = text[..start].trim_end();
        let next = text[index..].trim_start().as_bytes().first().copied();
        let used_as_object =
            matches!(next, Some(b'.' | b'[' | b'(' | b'?' | b'=')) || before.ends_with('.') || before.ends_with("?.");
        if !lists.contains(name) || used_as_object || name.as_bytes()[0].is_ascii_digit() {
            out.push_str(name);
            continue;
        }
        count += 1;
        if out.trim_end().ends_with('!') && !out.trim_end().ends_with("!!") {
            out.push_str(&format!("{name}?.length"));
        } else {
            out.push_str(&format!("({name}?.length > 0)"));
        }
    }
    (out, count)
}

fn line_of(text: &str, offset: usize) -> u32 {
    u32::try_from(text[..offset.min(text.len())].matches('\n').count() + 1).unwrap_or(u32::MAX)
}

/// The inside of each `${...}` of `text`, as byte ranges.
fn dollar_spans(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] == b'$' && bytes[index + 1] == b'{' && (index == 0 || bytes[index - 1] != b'\\') {
            let start = index + 2;
            let mut depth = 1;
            let mut end = start;
            let mut quote = 0u8;
            while end < bytes.len() {
                let byte = bytes[end];
                if quote != 0 {
                    if byte == b'\\' {
                        end += 1;
                    } else if byte == quote {
                        quote = 0;
                    }
                } else if byte == b'\'' || byte == b'"' {
                    quote = byte;
                } else if byte == b'{' {
                    depth += 1;
                } else if byte == b'}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                end += 1;
            }
            let end = end.min(bytes.len());
            out.push((start, end));
            index = end;
        }
        index += 1;
    }
    out
}

/// Rewrite one expression or script. Returns the text, what was rewritten, and why any place was left.
pub fn rewrite_expression(text: &str) -> (String, Vec<RewriteKind>, Vec<&'static str>) {
    rewrite_expression_with(text, &BTreeSet::new())
}

/// [`rewrite_expression`] knowing the found-list names of the file.
pub fn rewrite_expression_with(text: &str, lists: &BTreeSet<String>) -> (String, Vec<RewriteKind>, Vec<&'static str>) {
    let (text, mut kinds, mut left) = rewrite_core(text);
    let mut current = text;
    let (next, count) = string_equals(&current);
    if count > 0 {
        kinds.push(RewriteKind::StringEquals);
    }
    current = next;
    let (next, count, skipped) = casts(&current);
    if count > 0 {
        kinds.push(RewriteKind::Cast);
    }
    if skipped > 0 {
        left.push("cast of an expression that is not a name chain, or under a tighter operator");
    }
    current = next;
    let (next, count) = in_lists(&current);
    if count > 0 {
        kinds.push(RewriteKind::InList);
    }
    current = next;
    let (next, count) = list_includes(&current, lists);
    if count > 0 {
        kinds.push(RewriteKind::ListIncludes);
    }
    current = next;
    let (next, count) = empty_collections(&current);
    if count > 0 {
        kinds.push(RewriteKind::EmptyCollection);
    }
    current = next;
    let (next, count) = quoted_properties(&current);
    if count > 0 {
        kinds.push(RewriteKind::QuotedProperty);
    }
    current = next;
    let (next, count, skipped) = gstrings(&current);
    if count > 0 {
        kinds.push(RewriteKind::GString);
    }
    if skipped > 0 {
        left.push("GString with a bare $name, a nested double quote, or a backtick");
    }
    kinds.sort();
    kinds.dedup();
    (next, kinds, left)
}

fn rewrite_core(text: &str) -> (String, Vec<RewriteKind>, Vec<&'static str>) {
    let mut kinds = BTreeSet::new();
    let mut left = Vec::new();
    let (literals, maps, plus_left) = convert_literals_counted(text);
    if maps > 0 {
        kinds.insert(RewriteKind::MapLiteral);
    }
    if plus_left > 0 {
        left.push("map plus: `x + [a: b]` is a copy with entries in Groovy and text in JavaScript");
    }
    let (context, plus) = context_plus(&literals);
    if plus > 0 {
        kinds.insert(RewriteKind::ContextPlus);
        // The literal in `context + [...]` was a map rewrite only to make the copy.
        if maps == plus {
            kinds.remove(&RewriteKind::MapLiteral);
        }
    }
    let (elvis, rewritten, skipped) = elvis(&context);
    if rewritten > 0 {
        kinds.insert(RewriteKind::Elvis);
    }
    if skipped > 0 {
        left.push("Elvis before a ternary");
    }
    (elvis, kinds.into_iter().collect(), left)
}

/// Every Groovy list and map literal of `text` in JavaScript: maps become objects, lists stay arrays. Strings and
/// comments are copied as they are, and an index (`a[0]`) is not a literal.
pub fn convert_literals(text: &str) -> String {
    convert_literals_counted(text).0
}

/// The converted text, the maps rewritten, and the maps left because a `+` that is not `context +` comes before them.
fn convert_literals_counted(text: &str) -> (String, usize, usize) {
    let mut counts = (0, 0);
    let out = convert_range(text, &mut counts);
    (out, counts.0, counts.1)
}

/// Whether the text before a map literal ends in `+`, and whether the operand of that `+` is `context`.
fn after_plus(before: &str) -> Option<bool> {
    let left = before.trim_end().strip_suffix('+')?;
    if left.ends_with('+') {
        return None;
    }
    let left = left.trim_end();
    let word: String = left
        .chars()
        .rev()
        .take_while(|c| is_ident(u8::try_from(*c).unwrap_or(b' ')) || *c == '.')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    Some(word == "context")
}

fn convert_range(text: &str, counts: &mut (usize, usize)) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(end) = skip_string_or_comment(text, index) {
            out.push_str(&text[index..end]);
            index = end;
            continue;
        }
        if byte == b'[' && !is_index(&out) {
            if let Some(close) = matching(text, index, b'[', b']') {
                let inner = &text[index + 1..close];
                let converted = convert_range(inner, counts);
                if is_map(inner) && after_plus(&out) == Some(false) {
                    // `x + [a: b]` needs the operand's type to convert; leave it whole.
                    counts.1 += 1;
                    out.push('[');
                    out.push_str(&converted);
                    out.push(']');
                } else if is_map(inner) {
                    counts.0 += 1;
                    out.push('{');
                    out.push_str(&dynamic_keys(&converted));
                    out.push('}');
                } else {
                    out.push('[');
                    out.push_str(&converted);
                    out.push(']');
                }
                index = close + 1;
                continue;
            }
        }
        let ch = text[index..].chars().next().unwrap_or(' ');
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

/// The end of the string literal or comment at `index`, when one starts there.
fn skip_string_or_comment(text: &str, index: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let byte = bytes[index];
    if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
        return Some(text[index..].find('\n').map_or(text.len(), |end| index + end));
    }
    if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
        return Some(
            text[index + 2..]
                .find("*/")
                .map_or(text.len(), |end| index + 2 + end + 2),
        );
    }
    if byte == b'\'' || byte == b'"' || byte == b'`' {
        for quote in ["'''", "\"\"\""] {
            if text[index..].starts_with(quote) {
                return Some(
                    text[index + 3..]
                        .find(quote)
                        .map_or(text.len(), |end| index + 3 + end + 3),
                );
            }
        }
        let mut end = index + 1;
        while end < bytes.len() {
            if bytes[end] == b'\\' {
                end += 2;
                continue;
            }
            if bytes[end] == byte {
                return Some(end + 1);
            }
            end += 1;
        }
        return Some(bytes.len());
    }
    None
}

/// Whether a `[` that follows `before` indexes the value before it rather than starting a literal.
fn is_index(before: &str) -> bool {
    let trimmed = before.trim_end();
    let Some(last) = trimmed.chars().last() else {
        return false;
    };
    if last == ')' || last == ']' || last == '}' {
        return trimmed.len() == before.len();
    }
    if !(last.is_alphanumeric() || last == '_' || last == '$') {
        return false;
    }
    // A keyword before `[` starts a literal: `return [a: 1]`, `in [1, 2]`.
    let word: String = trimmed
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    !matches!(
        word.as_str(),
        "return" | "in" | "case" | "else" | "typeof" | "yield" | "new" | "of"
    )
}

/// The index of the bracket that closes the one at `open`, skipping strings and nested brackets of every kind.
fn matching(text: &str, open: usize, opening: u8, closing: u8) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut index = open;
    while index < bytes.len() {
        if index > open {
            if let Some(end) = skip_string_or_comment(text, index) {
                index = end;
                continue;
            }
        }
        let byte = bytes[index];
        if byte == opening || (byte != opening && matches!(byte, b'[' | b'(' | b'{')) {
            depth += 1;
        } else if byte == closing || matches!(byte, b']' | b')' | b'}') {
            depth -= 1;
            if depth == 0 {
                return (byte == closing).then_some(index);
            }
        }
        index += 1;
    }
    None
}

/// A Groovy map literal: `:` alone, or a first entry that is a key and a colon. A ternary in a list is not a key.
fn is_map(inner: &str) -> bool {
    let trimmed = inner.trim();
    if trimmed == ":" {
        return true;
    }
    let bytes = trimmed.as_bytes();
    let key_end = if let Some(end) = bytes
        .first()
        .filter(|byte| matches!(**byte, b'\'' | b'"'))
        .and_then(|_| skip_string_or_comment(trimmed, 0))
    {
        end
    } else if bytes.first() == Some(&b'(') {
        match matching(trimmed, 0, b'(', b')') {
            Some(close) => close + 1,
            None => return false,
        }
    } else {
        let end = bytes
            .iter()
            .position(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'$'))
            .unwrap_or(bytes.len());
        if end == 0 || bytes[0].is_ascii_digit() {
            return false;
        }
        end
    };
    let rest = trimmed[key_end..].trim_start();
    rest.starts_with(':') && !rest.starts_with("::")
}

/// `[:]` is `{}`, and a key in parentheses is a computed key.
fn dynamic_keys(inner: &str) -> String {
    if inner.trim() == ":" {
        return String::new();
    }
    let bytes = inner.as_bytes();
    let mut out = String::with_capacity(inner.len());
    let mut index = 0;
    let mut entry_start = true;
    let mut depth = 0i32;
    while index < bytes.len() {
        if let Some(end) = skip_string_or_comment(inner, index) {
            out.push_str(&inner[index..end]);
            index = end;
            entry_start = false;
            continue;
        }
        let byte = bytes[index];
        if entry_start && byte.is_ascii_whitespace() {
            out.push(byte as char);
            index += 1;
            continue;
        }
        if entry_start && depth == 0 && byte == b'(' {
            if let Some(close) = matching(inner, index, b'(', b')') {
                if inner[close + 1..].trim_start().starts_with(':') {
                    out.push('[');
                    out.push_str(&inner[index + 1..close]);
                    out.push(']');
                    index = close + 1;
                    entry_start = false;
                    continue;
                }
            }
        }
        entry_start = false;
        match byte {
            b'[' | b'(' | b'{' => depth += 1,
            b']' | b')' | b'}' => depth -= 1,
            b',' if depth == 0 => entry_start = true,
            _ => {}
        }
        let ch = inner[index..].chars().next().unwrap_or(' ');
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

/// `context + {…}`, after the literal pass made the map an object, becomes `Object.assign({}, context, {…})`.
fn context_plus(text: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut count = 0;
    let mut index = 0;
    let bytes = text.as_bytes();
    while index < bytes.len() {
        if let Some(end) = skip_string_or_comment(text, index) {
            out.push_str(&text[index..end]);
            index = end;
            continue;
        }
        if text[index..].starts_with("context")
            && (index == 0 || !is_ident(bytes[index - 1]))
            && !bytes.get(index + 7).is_some_and(|byte| is_ident(*byte))
        {
            let after = &text[index + 7..];
            let plus = after.len() - after.trim_start().len();
            if after.trim_start().starts_with('+') {
                let rest_at = index + 7 + plus + 1;
                let brace = rest_at + (text[rest_at..].len() - text[rest_at..].trim_start().len());
                if bytes.get(brace) == Some(&b'{') {
                    if let Some(close) = matching(text, brace, b'{', b'}') {
                        out.push_str("Object.assign({}, context, ");
                        out.push_str(&text[brace..=close]);
                        out.push(')');
                        count += 1;
                        index = close + 1;
                        continue;
                    }
                }
            }
        }
        let ch = text[index..].chars().next().unwrap_or(' ');
        out.push(ch);
        index += ch.len_utf8();
    }
    (out, count)
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

/// `a ?: b` as `a || b`. A `(?:` is a regular expression group and is not Elvis. An Elvis followed by a ternary at
/// its depth is left, because `a ?: b ? c : d` is `a ?: (b ? c : d)` in Groovy and `(a || b) ? c : d` in JavaScript.
fn elvis(text: &str) -> (String, usize, usize) {
    let mut out = String::with_capacity(text.len());
    let mut rewritten = 0;
    let mut skipped = 0;
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if let Some(end) = skip_string_or_comment(text, index) {
            out.push_str(&text[index..end]);
            index = end;
            continue;
        }
        if text[index..].starts_with("?:") && !out.ends_with('(') {
            if ternary_follows(&text[index + 2..]) {
                skipped += 1;
                out.push_str("?:");
            } else {
                rewritten += 1;
                out.push_str("||");
            }
            index += 2;
            continue;
        }
        let ch = text[index..].chars().next().unwrap_or(' ');
        out.push(ch);
        index += ch.len_utf8();
    }
    (out, rewritten, skipped)
}

/// Whether a `?` of a ternary comes before the operand ends: a closing bracket, a comma or a statement end.
fn ternary_follows(rest: &str) -> bool {
    let bytes = rest.as_bytes();
    let mut depth = 0i32;
    let mut index = 0;
    while index < bytes.len() {
        if let Some(end) = skip_string_or_comment(rest, index) {
            index = end;
            continue;
        }
        match bytes[index] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            b',' | b';' | b'\n' if depth == 0 => return false,
            b'?' if depth == 0 => {
                let next = bytes.get(index + 1).copied();
                if next == Some(b':') {
                    index += 2;
                    continue;
                }
                if next != Some(b'.') && next != Some(b'?') {
                    return true;
                }
            }
            _ => {}
        }
        index += 1;
    }
    false
}

fn one_line(text: &str) -> String {
    let joined: Vec<&str> = text.split_whitespace().collect();
    let line = joined.join(" ");
    if line.chars().count() > 240 {
        let mut clipped: String = line.chars().take(240).collect();
        clipped.push_str("...");
        clipped
    } else {
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(text: &str) -> String {
        rewrite_expression(text).0
    }

    #[test]
    fn map_literals_become_objects_and_lists_and_indexes_stay() {
        assert_eq!(rewrite("[a: 1, b: 'x']"), "{a: 1, b: 'x'}");
        assert_eq!(rewrite("[:]"), "{}");
        assert_eq!(rewrite("[1, 2, [a:3]]"), "[1, 2, {a:3}]");
        assert_eq!(rewrite("list[0].x"), "list[0].x");
        assert_eq!(rewrite("map['k']"), "map['k']");
        assert_eq!(rewrite("[(name): v, 'q r': 2]"), "{[name]: v, 'q r': 2}");
        assert_eq!(rewrite("[x ? 1 : 2, y]"), "[x ? 1 : 2, y]");
        assert_eq!(rewrite("f('[a: 1]', [b: \"[c:2]\"])"), "f('[a: 1]', {b: \"[c:2]\"})");
        assert_eq!(rewrite("return [ok: true]"), "return {ok: true}");
        assert_eq!(rewrite("[a: m[0], b: [c: f(x)[1]]]"), "{a: m[0], b: {c: f(x)[1]}}");
    }

    #[test]
    fn context_plus_becomes_an_object_assign() {
        let (text, kinds, _) = rewrite_expression("context + [flattenDocument: false]");
        assert_eq!(text, "Object.assign({}, context, {flattenDocument: false})");
        assert_eq!(kinds, vec![RewriteKind::ContextPlus]);
        assert_eq!(
            rewrite("context+[a:1, b:[c:2]]"),
            "Object.assign({}, context, {a:1, b:{c:2}})"
        );
        let (text, kinds, left) = rewrite_expression("baseMap + [a: 1]");
        assert_eq!(text, "baseMap + [a: 1]");
        assert!(kinds.is_empty());
        assert_eq!(left.len(), 1);
        assert_eq!(rewrite("mycontext + [a: 1]"), "mycontext + [a: 1]");
        assert_eq!(rewrite("x = [a: 1] + 2"), "x = {a: 1} + 2");
    }

    #[test]
    fn elvis_becomes_or_unless_a_ternary_follows() {
        assert_eq!(rewrite("a ?: 'x'"), "a || 'x'");
        assert_eq!(
            rewrite("${product?.productName ?: productId}"),
            "${product?.productName || productId}"
        );
        assert_eq!(rewrite("a?:b?:c"), "a||b||c");
        assert_eq!(rewrite("'?:' + x"), "'?:' + x");
        assert_eq!(rewrite("s.replaceAll(/(?:x)/, '')"), "s.replaceAll(/(?:x)/, '')");
        let (text, kinds, left) = rewrite_expression("a ?: b ? c : d");
        assert_eq!(text, "a ?: b ? c : d");
        assert!(kinds.is_empty());
        assert_eq!(left, vec!["Elvis before a ternary"]);
        assert_eq!(rewrite("f(a ?: b, c ? d : e)"), "f(a || b, c ? d : e)");
    }

    #[test]
    fn xml_places_are_converted_and_the_rest_is_copied() {
        let xml = r#"<services>
    <!-- a ?: b in a comment -->
    <service verb="get" noun="Thing">
        <description>Prose with ?: stays</description>
        <actions>
            <service-call name="x.y#Z" in-map="context + [flattenDocument:false]" out-map="context"/>
            <set field="name" from="product?.productName ?: productId"/>
            <message>Name ${name ?: 'none'}</message>
            <script><![CDATA[def m = [a: 1]; out = m.a ?: 2]]></script>
        </actions>
    </service>
</services>"#;
        let file = convert_xml(xml);
        assert!(
            file.text
                .contains("in-map=\"Object.assign({}, context, {flattenDocument:false})\""),
            "{}",
            file.text
        );
        assert!(
            file.text.contains("from=\"product?.productName || productId\""),
            "{}",
            file.text
        );
        assert!(file.text.contains("Name ${name || 'none'}"), "{}", file.text);
        assert!(file.text.contains("def m = {a: 1}; out = m.a || 2"), "{}", file.text);
        assert!(file.text.contains("<!-- a ?: b in a comment -->"), "{}", file.text);
        assert!(file.text.contains("Prose with ?: stays"), "{}", file.text);
        assert_eq!(file.rewrites.len(), 4);
        assert_eq!(file.rewrites[0].line, 6);
    }

    #[test]
    fn an_entity_field_default_is_an_expression() {
        let file = convert_xml(
            r#"<entity><field name="sequenceNum" type="number-integer" default="fieldSeqId as int"/></entity>"#,
        );
        assert!(
            file.text
                .contains(r#"default="(fieldSeqId == null ? null : Math.trunc(toDecimal(fieldSeqId).toNumber()))""#),
            "{}",
            file.text
        );
    }

    #[test]
    fn a_grammar_file_becomes_javascript_the_edi_handler_reads() {
        let groovy = "// File Structure\n[\n    [ID:'ISA',MIN:0,MAX:99999,\n        LEVEL: [\n            [ID:'GS',MIN:0,MAX:99999, LEVEL: [\n                    [ID:'ST',MIN:0,MAX:99999],\n                ]\n            ],\n            [ID:'IEA',MIN:1,MAX:1]\n        ]\n    ]\n]\n";
        assert!(is_grammar(groovy));
        let js = convert_literals(groovy);
        assert!(js.contains("{ID:'ISA',MIN:0,MAX:99999,"), "{js}");
        assert!(js.contains("{ID:'ST',MIN:0,MAX:99999},"), "{js}");
        super::super::parse_js(&js).unwrap();
        assert!(!is_grammar("def x = 1\n[a]"));
    }

    #[test]
    fn a_tree_converts_grammars_and_their_locations_and_writes_only_when_asked() {
        let dir = std::env::temp_dir().join(format!("moqui-groovy-convert-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("service/edi")).unwrap();
        std::fs::write(dir.join("service/edi/X12Test.groovy"), "[[ID:'ST', MIN:1, MAX:1]]\n").unwrap();
        let services = r#"<services><service verb="a" noun="B"><in-parameters>
            <parameter name="bodyLocation" default-value="component://c/service/edi/X12Test.groovy"/></in-parameters>
            <actions><set field="x" from="y ?: 1"/></actions></service></services>"#;
        std::fs::write(dir.join("service/EdiServices.xml"), services).unwrap();
        let report = convert_tree(std::slice::from_ref(&dir), false).unwrap();
        assert_eq!(report.count(RewriteKind::GrammarFile), 1);
        assert_eq!(report.count(RewriteKind::GrammarLocation), 1);
        assert_eq!(report.count(RewriteKind::Elvis), 1);
        assert!(!dir.join("service/edi/X12Test.js").exists(), "a dry run writes nothing");
        assert!(report.render().contains("X12Test.js"));
        convert_tree(std::slice::from_ref(&dir), true).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("service/edi/X12Test.js")).unwrap(),
            "[{ID:'ST', MIN:1, MAX:1}]\n"
        );
        let written = std::fs::read_to_string(dir.join("service/EdiServices.xml")).unwrap();
        assert!(written.contains("component://c/service/edi/X12Test.js\""), "{written}");
        assert!(written.contains("from=\"y || 1\""), "{written}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_found_list_in_a_condition_is_tested_for_rows() {
        let xml = r#"<actions>
            <entity-find entity-name="a.B" list="fooList"><econdition field-name="x"/></entity-find>
            <if condition="fooList"><log message="x"/></if>
            <if condition="!fooList &amp;&amp; otherList"><log message="x"/></if>
            <if condition="fooList.size() &gt; 1 || fooList[0] || 'fooList'"><log message="x"/></if>
        </actions>"#;
        let converted = convert_xml(xml);
        assert!(
            converted.text.contains(r#"<if condition="(fooList?.length > 0)">"#),
            "{}",
            converted.text
        );
        assert!(
            converted
                .text
                .contains(r#"<if condition="!fooList?.length &amp;&amp; otherList">"#),
            "{}",
            converted.text
        );
        assert!(
            converted
                .text
                .contains(r#"<if condition="fooList.size() &gt; 1 || fooList[0] || 'fooList'">"#),
            "{}",
            converted.text
        );
        assert_eq!(
            converted
                .rewrites
                .iter()
                .filter(|rewrite| rewrite.kinds == vec![RewriteKind::EmptyList])
                .count(),
            2
        );
    }

    #[test]
    fn string_equals_casts_includes_and_empty_collections() {
        let lists: BTreeSet<String> = ["orderList".to_string()].into_iter().collect();
        let rewrite = |text: &str| rewrite_expression_with(text, &lists).0;
        assert_eq!(
            rewrite("'DEBIT'.equals(rootId) ? 'N' : 'Y'"),
            "('DEBIT' === rootId) ? 'N' : 'Y'"
        );
        assert_eq!(
            rewrite("party.partyTypeEnumId.equals(\"PtyPerson\")"),
            "(party.partyTypeEnumId === \"PtyPerson\")"
        );
        assert_eq!(rewrite("foo(a).equals('x')"), "foo(a).equals('x')", "not a name chain");
        assert_eq!(rewrite("curYear as String"), "curYear?.toString()");
        assert_eq!(rewrite("(curYear + 1) as String"), "(curYear + 1)?.toString()");
        assert_eq!(
            rewrite("item.quantity as BigDecimal"),
            "(item.quantity == null ? null : toDecimal(item.quantity))"
        );
        assert_eq!(
            rewrite("x as int"),
            "(x == null ? null : Math.trunc(toDecimal(x).toNumber()))"
        );
        assert_eq!(
            rewrite("if ('A'.equals(x)) y"),
            "if ('A' === x) y",
            "already in parentheses"
        );
        assert_eq!(
            rewrite("(total as BigDecimal).add(1)"),
            "(total == null ? null : toDecimal(total)).add(1)"
        );
        assert_eq!(
            rewrite("'n' + count as String"),
            "'n' + count as String",
            "the sum is cast in Groovy"
        );
        assert_eq!(rewrite("x = count as String"), "x = count?.toString()");
        let (text, _, left) = rewrite_expression_with("foo(a) as BigDecimal", &lists);
        assert_eq!(text, "foo(a) as BigDecimal");
        assert_eq!(
            left,
            vec!["cast of an expression that is not a name chain, or under a tighter operator"]
        );
        assert_eq!(rewrite("'a as String'"), "'a as String'", "strings are left");
        assert_eq!(
            rewrite("asset.'mantle.product.Product'"),
            "asset['mantle.product.Product']"
        );
        assert_eq!(
            rewrite("srs?.'Origin#mantle.facility.Facility'?.facilityName"),
            "srs?.['Origin#mantle.facility.Facility']?.facilityName"
        );
        assert_eq!(
            rewrite("x + '.' + 'y'"),
            "x + '.' + 'y'",
            "a string after a string is not a property"
        );
        assert_eq!(rewrite("\"${a} 00:00:00.000\""), "`${a} 00:00:00.000`");
        assert_eq!(rewrite("\"plain\""), "\"plain\"", "no interpolation");
        assert_eq!(rewrite("\"\"\"${a}\n${b}\"\"\""), "`${a}\n${b}`");
        let (text, _, left) = rewrite_expression_with("\"Hi $name\"", &lists);
        assert_eq!(text, "\"Hi $name\"");
        assert_eq!(
            left,
            vec!["GString with a bare $name, a nested double quote, or a backtick"]
        );
        assert_eq!(
            rewrite("orderList.contains(id) || 'ab'.contains(c) || other.contains(d)"),
            "orderList.includes(id) || 'ab'.includes(c) || other.contains(d)"
        );
        assert_eq!(
            rewrite("x = new ArrayList<String>(); m = new LinkedHashMap(); n = new HashMap(map)"),
            "x = []; m = {}; n = new HashMap(map)"
        );
    }

    #[test]
    fn multi_line_and_nested_maps_become_objects() {
        assert_eq!(
            rewrite("[\n    a: 1,\n    b: [c: 2,\n         d: [e: 3]],\n    (k): v\n]"),
            "{\n    a: 1,\n    b: {c: 2,\n         d: {e: 3}},\n    [k]: v\n}"
        );
        assert_eq!(
            rewrite("[a: [1, 2,\n  3], b: [:],\n // note: [x: 1]\n c: 'x']"),
            "{a: [1, 2,\n  3], b: {},\n // note: [x: 1]\n c: 'x'}"
        );
        let once = rewrite("[\n a: 1,\n b: [c: 2]\n]");
        assert_eq!(rewrite(&once), once, "idempotent");
    }

    #[test]
    fn in_a_list_literal_becomes_includes() {
        assert_eq!(rewrite("statusId in ['A', 'B']"), "['A', 'B'].includes(statusId)");
        assert_eq!(rewrite("!(statusId in ['A', 'B'])"), "!(['A', 'B'].includes(statusId))");
        assert_eq!(
            rewrite("statusChanged &amp;&amp; payment.statusId in ['A','B'] &amp;&amp; x == 1"),
            "statusChanged &amp;&amp; ['A','B'].includes(payment.statusId) &amp;&amp; x == 1"
        );
        assert_eq!(
            rewrite("a && (b?.c in ['A']) || d in [1, 2]"),
            "a && (['A'].includes(b?.c)) || [1, 2].includes(d)"
        );
        assert_eq!(rewrite("x.get('k') in ['A']"), "['A'].includes(x.get('k'))");
        assert_eq!(
            rewrite("ok = oldStatusId in [\n 'A',\n 'B']"),
            "ok = [\n 'A',\n 'B'].includes(oldStatusId)"
        );
        assert_eq!(rewrite("c ? x in ['A'] : y"), "c ? ['A'].includes(x) : y");
        assert_eq!(rewrite("return x in ['A']"), "return ['A'].includes(x)");
        // Left alone: other precedence, loops, non-literals, strings.
        for same in [
            "!x in ['A']",
            "a + x in ['A']",
            "a &lt; x in ['A']",
            "for (x in ['A', 'B']) { f(x) }",
            "for (String x in ['A', 'B']) { f(x) }",
            "x in someList",
            "x in {a: 1}",
            "'x in [1]'",
            "x in ['A'].size()",
            "// x in ['A']",
        ] {
            assert_eq!(rewrite(same), same);
        }
        let once = rewrite("!(a in ['A']) && b in ['B']");
        assert_eq!(rewrite(&once), once, "idempotent");
        let (_, kinds, _) = rewrite_expression("x in ['A']");
        assert_eq!(kinds, vec![RewriteKind::InList]);
    }

    #[test]
    fn more_cast_operand_shapes() {
        assert_eq!(
            rewrite("(convertDurOut.convertedAmount || 60) as int"),
            "((convertDurOut.convertedAmount || 60) == null ? null : Math.trunc(toDecimal((convertDurOut.convertedAmount || 60)).toNumber()))"
        );
        assert_eq!(
            rewrite("f(((a + b) as BigDecimal).divide(c, 3))"),
            "f(((a + b) == null ? null : toDecimal((a + b))).divide(c, 3))"
        );
        assert_eq!(
            rewrite("x = list[0].qty as BigDecimal"),
            "x = (list[0].qty == null ? null : toDecimal(list[0].qty))"
        );
        assert_eq!(rewrite("x = foo(a).bar as String"), "x = foo(a).bar?.toString()");
        assert_eq!(rewrite("x = a.b(c)[0]?.d as String"), "x = a.b(c)[0]?.d?.toString()");
        // A call would run twice, and a tighter operator before the operand moves the cast.
        assert_eq!(rewrite("x = foo(a) as int"), "x = foo(a) as int");
        assert_eq!(
            rewrite("x = (f(a) || 1) as BigDecimal"),
            "x = (f(a) || 1) as BigDecimal"
        );
        assert_eq!(rewrite("x = (a ? 1 : 2) as int"), "x = (a ? 1 : 2) as int");
        assert_eq!(rewrite("x = 1 + (a || 1) as int"), "x = 1 + (a || 1) as int");
        assert_eq!(rewrite("a &gt; b as int"), "a &gt; b as int");
        assert_eq!(rewrite("a &gt;= b as int"), "a &gt;= b as int");
        let once = rewrite("x = (a || 60) as int");
        assert_eq!(rewrite(&once), once, "idempotent");
    }
}
