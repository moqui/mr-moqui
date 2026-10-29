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

//! The lossless XML layer (plans/design-dev-aide.md 5.2, F7): structural operations on an XML text (set, remove and
//! rename an attribute; insert, remove, move and replace an element; set an element's text; wrap and unwrap) that answer
//! the smallest text edits, so the rest of the file keeps its comments, order, formatting and line endings and its git
//! diff is the change. The visual views of the dev-aide turn gestures into these, and agents' edits by element use them.
//!
//! New text follows the file: its indentation unit, its line ending, and attributes on the element's own line unless
//! that element already wraps its attributes. Each operation applies to the text the ones before it made; [`apply`]
//! answers the edits of each step, so an editor dispatches them as sequential changes of one transaction.
//!
//! An element is named by a [`Locator`]: a path of element child indexes from the root element (`[]` is the root), or a
//! selector such as `service[verb=place][noun=Order]/actions/entity-find[2]`, each step an element name with optional
//! `[attribute=value]` filters and an optional 1-based `[n]`, the first step matching the root element.

pub mod outline;
pub mod tokenize;

use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::error::Error;
pub use outline::{outline, OutlineNode};
pub use tokenize::{Doc, Kind};

/// An element of the text.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Locator {
    Path(Vec<usize>),
    Selector(String),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Position {
    Before,
    After,
    /// The first child of the element.
    First,
    /// The last child of the element.
    Last,
}

/// One operation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum Op {
    SetAttribute {
        at: Locator,
        name: String,
        value: String,
    },
    RemoveAttribute {
        at: Locator,
        name: String,
    },
    RenameAttribute {
        at: Locator,
        name: String,
        to: String,
    },
    /// Insert `xml` (one or more elements, as written) at `position` of the element `at`.
    Insert {
        at: Locator,
        position: Position,
        xml: String,
    },
    Remove {
        at: Locator,
    },
    /// Move the element `at` to `position` of the element `to` (named in the text before the move).
    Move {
        at: Locator,
        to: Locator,
        position: Position,
    },
    Replace {
        at: Locator,
        xml: String,
    },
    /// Set the text of an element with no element children; a CDATA section stays one.
    SetText {
        at: Locator,
        text: String,
    },
    /// Wrap the element in `xml`, an empty element such as `<container name="Box"/>`.
    Wrap {
        at: Locator,
        xml: String,
    },
    /// Put the element's children in its place.
    Unwrap {
        at: Locator,
    },
}

/// A replacement of the bytes `from..to` of a step's text by `text`. `from16` and `to16` are the same in UTF-16 code
/// units, as a browser editor counts.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Edit {
    pub from: usize,
    pub to: usize,
    pub text: String,
    pub from16: usize,
    pub to16: usize,
}

/// What [`apply`] made: the new text, and the edits of each operation on the text before it.
#[derive(Clone, Debug, Serialize)]
pub struct Applied {
    pub text: String,
    pub steps: Vec<Vec<Edit>>,
}

/// Apply `ops` to `text` in order.
///
/// # Errors
///
/// Returns an error naming the operation when the text does not parse, a locator names no element, or an operation
/// cannot apply (text set on an element with element children, a move into the element moved).
pub fn apply(text: &str, ops: &[Op]) -> Result<Applied, Error> {
    let mut current = text.to_string();
    let mut steps = Vec::with_capacity(ops.len());
    for (index, op) in ops.iter().enumerate() {
        let doc = Doc::parse(&current)?;
        let mut edits = edits_of(&doc, op).map_err(|err| Error::new(format!("operation {}: {err}", index + 1)))?;
        edits.sort_by_key(|edit| edit.range.start);
        if edits
            .windows(2)
            .any(|pair| matches!(pair, [a, b] if a.range.end > b.range.start))
        {
            return Err(Error::new(format!("operation {}: its edits overlap", index + 1)));
        }
        let mut next = String::with_capacity(current.len() + 64);
        let mut at = 0;
        let mut step = Vec::with_capacity(edits.len());
        for Splice { range, text } in edits {
            next.push_str(&current[at..range.start]);
            next.push_str(&text);
            at = range.end;
            step.push(Edit {
                from: range.start,
                to: range.end,
                from16: utf16_len(&current[..range.start]),
                to16: utf16_len(&current[..range.end]),
                text,
            });
        }
        next.push_str(&current[at..]);
        current = next;
        steps.push(step);
    }
    Ok(Applied { text: current, steps })
}

fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// One replacement an operation makes: the bytes `range` of the text before it become `text`.
struct Splice {
    range: Range<usize>,
    text: String,
}

fn splice(range: Range<usize>, text: impl Into<String>) -> Splice {
    Splice {
        range,
        text: text.into(),
    }
}

type Edits = Vec<Splice>;

fn edits_of(doc: &Doc<'_>, op: &Op) -> Result<Edits, Error> {
    let style = Style::of(doc.text);
    match op {
        Op::SetAttribute { at, name, value } => {
            let element = locate(doc, at)?;
            Ok(vec![set_attribute(doc, &style, element, name, value)])
        }
        Op::RemoveAttribute { at, name } => {
            let element = locate(doc, at)?;
            let attr = find_attr(doc, element, name)?;
            Ok(vec![remove_attribute(doc, element, attr)])
        }
        Op::RenameAttribute { at, name, to } => {
            let element = locate(doc, at)?;
            let attr = find_attr(doc, element, name)?;
            Ok(vec![splice(doc.nodes[element].attrs[attr].name.clone(), to.clone())])
        }
        Op::Insert { at, position, xml } => {
            let element = locate(doc, at)?;
            insert(doc, &style, element, *position, xml)
        }
        Op::Remove { at } => {
            let element = locate(doc, at)?;
            if doc.nodes[element].parent == Some(0) {
                return Err(Error::new("the root element cannot be removed"));
            }
            Ok(vec![remove(doc, element)])
        }
        Op::Move { at, to, position } => {
            let moved = locate(doc, at)?;
            let target = locate(doc, to)?;
            if within(doc, target, moved) {
                return Err(Error::new("an element cannot move into itself"));
            }
            let fragment = dedent(
                doc.slice(&doc.nodes[moved].span),
                &indent_of(doc, &style, moved),
                &style,
            );
            let mut edits = vec![remove(doc, moved)];
            edits.extend(insert(doc, &style, target, *position, &fragment)?);
            Ok(edits)
        }
        Op::Replace { at, xml } => {
            let element = locate(doc, at)?;
            let indent = indent_of(doc, &style, element);
            Ok(vec![splice(
                doc.nodes[element].span.clone(),
                reindent(xml, &indent, &style),
            )])
        }
        Op::SetText { at, text } => {
            let element = locate(doc, at)?;
            set_text(doc, &style, element, text)
        }
        Op::Wrap { at, xml } => {
            let element = locate(doc, at)?;
            wrap(doc, &style, element, xml)
        }
        Op::Unwrap { at } => {
            let element = locate(doc, at)?;
            unwrap(doc, &style, element)
        }
    }
}

/// The file's ways: its line ending and its indentation unit.
struct Style {
    eol: &'static str,
    unit: String,
}

impl Style {
    fn of(text: &str) -> Self {
        let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let mut unit = None;
        let mut tabs = 0;
        let mut spaced = 0;
        let mut smallest = usize::MAX;
        for line in text.lines() {
            let indent: String = line.chars().take_while(|ch| *ch == ' ' || *ch == '\t').collect();
            if indent.is_empty() || line.trim().is_empty() {
                continue;
            }
            if indent.starts_with('\t') {
                tabs += 1;
            } else {
                spaced += 1;
                smallest = smallest.min(indent.len());
            }
        }
        if tabs > spaced {
            unit = Some("\t".to_string());
        } else if spaced > 0 && (2..=8).contains(&smallest) {
            unit = Some(" ".repeat(smallest));
        }
        Self {
            eol,
            unit: unit.unwrap_or_else(|| "    ".to_string()),
        }
    }
}

fn locate(doc: &Doc<'_>, locator: &Locator) -> Result<usize, Error> {
    let root = doc.root().ok_or_else(|| Error::new("the text has no root element"))?;
    match locator {
        Locator::Path(path) => {
            let mut at = root;
            for (depth, index) in path.iter().enumerate() {
                at = doc
                    .elements(at)
                    .nth(*index)
                    .ok_or_else(|| Error::new(format!("no element at {:?}", &path[..=depth])))?;
            }
            Ok(at)
        }
        Locator::Selector(selector) => {
            let mut candidates = vec![0];
            for (depth, step) in selector.split('/').filter(|step| !step.is_empty()).enumerate() {
                let (name, filters, nth) = parse_step(step)?;
                let mut matched = Vec::new();
                for parent in &candidates {
                    let mut of_parent = doc.elements(*parent).filter(|child| {
                        doc.name(*child) == name
                            && filters
                                .iter()
                                .all(|(attr, value)| doc.attr(*child, attr) == Some(value.as_str()))
                    });
                    match nth {
                        Some(nth) => matched.extend(of_parent.nth(nth - 1)),
                        None => matched.extend(of_parent),
                    }
                }
                if matched.is_empty() {
                    return Err(Error::new(format!("no element for step {} of {selector}", depth + 1)));
                }
                candidates = matched;
            }
            match candidates.as_slice() {
                [one] if *one != 0 => Ok(*one),
                [] | [_] => Err(Error::new(format!("no element for {selector}"))),
                _ => Err(Error::new(format!(
                    "{selector} names {} elements; add a filter or [n]",
                    candidates.len()
                ))),
            }
        }
    }
}

/// One step of a selector: an element name, `[attribute=value]` filters, and an optional 1-based index.
type Step = (String, Vec<(String, String)>, Option<usize>);

/// `name[attr=value][2]` as its name, filters and 1-based index.
fn parse_step(step: &str) -> Result<Step, Error> {
    let name_end = step.find('[').unwrap_or(step.len());
    let name = step[..name_end].trim().to_string();
    let mut filters = Vec::new();
    let mut nth = None;
    let mut rest = &step[name_end..];
    while let Some(open) = rest.find('[') {
        let close = rest[open..]
            .find(']')
            .map(|at| open + at)
            .ok_or_else(|| Error::new(format!("unclosed [ in {step}")))?;
        let inner = rest[open + 1..close].trim();
        match inner.split_once('=') {
            Some((attr, value)) => filters.push((
                attr.trim().to_string(),
                value.trim().trim_matches(|ch| ch == '"' || ch == '\'').to_string(),
            )),
            None => {
                nth = Some(
                    inner
                        .parse::<usize>()
                        .ok()
                        .filter(|nth| *nth > 0)
                        .ok_or_else(|| Error::new(format!("[{inner}] in {step} is not a 1-based index")))?,
                );
            }
        }
        rest = &rest[close + 1..];
    }
    if name.is_empty() {
        return Err(Error::new(format!("a step with no element name: {step}")));
    }
    Ok((name, filters, nth))
}

fn find_attr(doc: &Doc<'_>, element: usize, name: &str) -> Result<usize, Error> {
    doc.nodes[element]
        .attrs
        .iter()
        .position(|attr| doc.slice(&attr.name) == name)
        .ok_or_else(|| Error::new(format!("<{}> has no attribute {name}", doc.name(element))))
}

fn within(doc: &Doc<'_>, node: usize, ancestor: usize) -> bool {
    let mut current = Some(node);
    while let Some(at) = current {
        if at == ancestor {
            return true;
        }
        current = doc.nodes[at].parent;
    }
    false
}

fn escape_attr(value: &str, quote: u8) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' if quote == b'"' => out.push_str("&quot;"),
            '\'' if quote == b'\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

fn escape_text(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Where the line of `at` starts.
fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |found| found + 1)
}

/// The indentation of an element: the whitespace before it on its line when nothing else is, else its parent's plus one
/// unit.
fn indent_of(doc: &Doc<'_>, style: &Style, element: usize) -> String {
    let start = doc.nodes[element].span.start;
    let before = &doc.text[line_start(doc.text, start)..start];
    if before.chars().all(|ch| ch == ' ' || ch == '\t') {
        return before.to_string();
    }
    match doc.nodes[element].parent.filter(|parent| *parent != 0) {
        Some(parent) => format!("{}{}", indent_of(doc, style, parent), style.unit),
        None => String::new(),
    }
}

/// `fragment` with its line endings the file's and each line after the first indented by `indent` more.
fn reindent(fragment: &str, indent: &str, style: &Style) -> String {
    let lines: Vec<&str> = fragment
        .trim()
        .split('\n')
        .map(|line| line.trim_end_matches('\r'))
        .collect();
    let mut out = String::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            out.push_str(style.eol);
            if !line.is_empty() {
                out.push_str(indent);
            }
        }
        out.push_str(line);
    }
    out
}

/// An element's text with `indent` taken off each line after the first, for putting it elsewhere.
fn dedent(text: &str, indent: &str, style: &Style) -> String {
    let mut out = String::new();
    for (index, line) in text.split('\n').map(|line| line.trim_end_matches('\r')).enumerate() {
        if index > 0 {
            out.push_str(style.eol);
            out.push_str(line.strip_prefix(indent).unwrap_or(line.trim_start()));
        } else {
            out.push_str(line);
        }
    }
    out
}

fn set_attribute(doc: &Doc<'_>, style: &Style, element: usize, name: &str, value: &str) -> Splice {
    let node = &doc.nodes[element];
    if let Some(attr) = node.attrs.iter().find(|attr| doc.slice(&attr.name) == name) {
        return splice(attr.value.clone(), escape_attr(value, attr.quote));
    }
    let written = format!("{name}=\"{}\"", escape_attr(value, b'"'));
    match node.attrs.last() {
        Some(last) => {
            // Attributes on lines of their own: the new one on its own line, lined up with the last.
            let name_line = line_start(doc.text, node.start_tag.start);
            let last_line = line_start(doc.text, last.span.start);
            if last_line != name_line {
                let column = &doc.text[last_line..last.span.start];
                let pad: String = column.chars().map(|ch| if ch == '\t' { '\t' } else { ' ' }).collect();
                splice(last.span.end..last.span.end, format!("{}{pad}{written}", style.eol))
            } else {
                splice(last.span.end..last.span.end, format!(" {written}"))
            }
        }
        None => splice(node.name.end..node.name.end, format!(" {written}")),
    }
}

fn remove_attribute(doc: &Doc<'_>, element: usize, attr: usize) -> Splice {
    let span = doc.nodes[element].attrs[attr].span.clone();
    // the whitespace before it goes with it
    let start = doc.text[..span.start].trim_end_matches([' ', '\t', '\r', '\n']).len();
    splice(start..span.end, "")
}

/// The edit that removes an element: its whole line when it is alone on it, else just its span.
fn remove(doc: &Doc<'_>, element: usize) -> Splice {
    let span = doc.nodes[element].span.clone();
    let start = line_start(doc.text, span.start);
    let before = &doc.text[start..span.start];
    let line_end = doc.text[span.end..].find('\n').map(|at| span.end + at + 1);
    let after = &doc.text[span.end..line_end.unwrap_or(doc.text.len())];
    if before.trim().is_empty() && after.trim().is_empty() {
        let end = line_end.unwrap_or(doc.text.len());
        // the last line of the text: take the line break before it instead
        if line_end.is_none() && start > 0 {
            let previous = doc.text[..start - 1].trim_end_matches('\r').len();
            return splice(previous..end, "");
        }
        return splice(start..end, "");
    }
    splice(span, "")
}

fn insert(doc: &Doc<'_>, style: &Style, element: usize, position: Position, xml: &str) -> Result<Edits, Error> {
    let node = &doc.nodes[element];
    match position {
        Position::Before | Position::After => {
            if node.parent == Some(0) {
                return Err(Error::new("nothing goes before or after the root element"));
            }
            let indent = indent_of(doc, style, element);
            let fragment = reindent(xml, &indent, style);
            let start = node.span.start;
            let alone = doc.text[line_start(doc.text, start)..start].trim().is_empty();
            Ok(vec![if position == Position::Before {
                if alone {
                    splice(start..start, format!("{fragment}{}{indent}", style.eol))
                } else {
                    splice(start..start, format!("{fragment} "))
                }
            } else if alone {
                splice(node.span.end..node.span.end, format!("{}{indent}{fragment}", style.eol))
            } else {
                splice(node.span.end..node.span.end, format!(" {fragment}"))
            }])
        }
        Position::First | Position::Last => {
            let children: Vec<usize> = doc.elements(element).collect();
            let anchor = if position == Position::First {
                children.first()
            } else {
                children.last()
            };
            if let Some(anchor) = anchor {
                return insert(
                    doc,
                    style,
                    *anchor,
                    if position == Position::First {
                        Position::Before
                    } else {
                        Position::After
                    },
                    xml,
                );
            }
            let indent = indent_of(doc, style, element);
            let inner = format!("{indent}{}", style.unit);
            let fragment = reindent(xml, &inner, style);
            if node.self_closing {
                // `<a x="1"/>` becomes `<a x="1">` + the children + `</a>`
                let close = node.start_tag.end - 2;
                let trimmed = doc.text[..close].trim_end_matches([' ', '\t']).len();
                let name = doc.name(element);
                return Ok(vec![splice(
                    trimmed..node.start_tag.end,
                    format!(">{eol}{inner}{fragment}{eol}{indent}</{name}>", eol = style.eol),
                )]);
            }
            let content = doc.slice(&node.content);
            if content.trim().is_empty() {
                Ok(vec![splice(
                    node.content.clone(),
                    format!("{eol}{inner}{fragment}{eol}{indent}", eol = style.eol),
                )])
            } else if position == Position::First {
                Ok(vec![splice(node.content.start..node.content.start, fragment)])
            } else {
                Ok(vec![splice(node.content.end..node.content.end, fragment)])
            }
        }
    }
}

fn set_text(doc: &Doc<'_>, style: &Style, element: usize, text: &str) -> Result<Edits, Error> {
    let node = &doc.nodes[element];
    if doc.elements(element).next().is_some() {
        return Err(Error::new(format!(
            "<{}> has element children; its text is not one value",
            doc.name(element)
        )));
    }
    let cdata = node
        .children
        .iter()
        .find(|child| doc.nodes[**child].kind == Kind::CData)
        .copied();
    if let Some(cdata) = cdata.filter(|_| !text.contains("]]>")) {
        let span = &doc.nodes[cdata].span;
        return Ok(vec![splice(span.start + 9..span.end - 3, text)]);
    }
    let escaped = escape_text(text);
    if node.self_closing {
        let close = node.start_tag.end - 2;
        let trimmed = doc.text[..close].trim_end_matches([' ', '\t']).len();
        let name = doc.name(element);
        return Ok(vec![splice(
            trimmed..node.start_tag.end,
            format!(">{escaped}</{name}>"),
        )]);
    }
    let _ = style;
    Ok(vec![splice(node.content.clone(), escaped)])
}

fn wrap(doc: &Doc<'_>, style: &Style, element: usize, xml: &str) -> Result<Edits, Error> {
    if doc.nodes[element].parent == Some(0) {
        return Err(Error::new("the root element cannot be wrapped"));
    }
    let wrapper = Doc::parse(xml.trim())?;
    let root = wrapper
        .root()
        .ok_or_else(|| Error::new("the wrapper is not an element"))?;
    let tag = &wrapper.nodes[root];
    if !tag.self_closing {
        return Err(Error::new(
            "the wrapper is an empty element, such as <container name=\"Box\"/>",
        ));
    }
    let open = wrapper.text[tag.start_tag.start..tag.start_tag.end - 2]
        .trim_end()
        .to_string();
    let name = wrapper.name(root);
    let indent = indent_of(doc, style, element);
    let inner = format!("{indent}{}", style.unit);
    let body = dedent(doc.slice(&doc.nodes[element].span), &indent, style);
    Ok(vec![splice(
        doc.nodes[element].span.clone(),
        format!(
            "{open}>{eol}{inner}{}{eol}{indent}</{name}>",
            reindent(&body, &inner, style),
            eol = style.eol
        ),
    )])
}

fn unwrap(doc: &Doc<'_>, style: &Style, element: usize) -> Result<Edits, Error> {
    let node = &doc.nodes[element];
    if node.parent == Some(0) {
        return Err(Error::new("the root element cannot be unwrapped"));
    }
    if node.self_closing || doc.slice(&node.content).trim().is_empty() {
        return Ok(vec![remove(doc, element)]);
    }
    let indent = indent_of(doc, style, element);
    let inner = format!("{indent}{}", style.unit);
    let content = doc.slice(&node.content);
    let lifted = dedent(content.trim(), &inner, style);
    Ok(vec![splice(node.span.clone(), reindent(&lifted, &indent, style))])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Vector {
        name: String,
        input: String,
        ops: Vec<Op>,
        #[serde(default)]
        expected: Option<String>,
        #[serde(default)]
        error: Option<String>,
    }

    /// The vectors the browser client is tested against too (crates/moqui-lib/testdata/xml-edit-vectors.json).
    #[test]
    fn the_vectors_pass() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/xml-edit-vectors.json");
        let vectors: Vec<Vector> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert!(vectors.len() >= 20, "{}", vectors.len());
        for vector in vectors {
            let result = apply(&vector.input, &vector.ops);
            match (&vector.expected, &vector.error) {
                (Some(expected), _) => {
                    let applied = result.unwrap_or_else(|err| panic!("{}: {err}", vector.name));
                    assert_eq!(&applied.text, expected, "{}", vector.name);
                    // the edits of the steps, applied in turn, give the same text
                    let mut text = vector.input.clone();
                    for step in &applied.steps {
                        for edit in step.iter().rev() {
                            text.replace_range(edit.from..edit.to, &edit.text);
                        }
                    }
                    assert_eq!(&text, expected, "{}: the edits", vector.name);
                }
                (None, Some(error)) => {
                    let err = result
                        .err()
                        .unwrap_or_else(|| panic!("{}: no error", vector.name))
                        .to_string();
                    assert!(err.contains(error.as_str()), "{}: {err}", vector.name);
                }
                (None, None) => panic!("{}: neither expected nor error", vector.name),
            }
        }
    }

    /// The steps WebrootCode.test.js applies to a CodeMirror state, as this layer makes them.
    #[test]
    fn the_steps_the_browser_test_applies() {
        let applied = apply(
            "<a t=\"\u{1F600}\">\n    <b/>\n</a>\n",
            &[
                Op::SetAttribute {
                    at: Locator::Path(vec![0]),
                    name: "x".into(),
                    value: "1".into(),
                },
                Op::Insert {
                    at: Locator::Path(vec![0]),
                    position: Position::After,
                    xml: "<c/>".into(),
                },
            ],
        )
        .unwrap();
        let firsts: Vec<(usize, usize, &str)> = applied
            .steps
            .iter()
            .map(|step| (step[0].from16, step[0].to16, step[0].text.as_str()))
            .collect();
        assert_eq!(firsts, [(17, 17, " x=\"1\""), (25, 25, "\n    <c/>")]);
        assert_eq!(applied.text, "<a t=\"\u{1F600}\">\n    <b x=\"1\"/>\n    <c/>\n</a>\n");
    }

    #[test]
    fn utf16_offsets_count_as_a_browser_does() {
        let applied = apply(
            "<a t=\"\u{1F600}\" x=\"1\"/>",
            &[Op::SetAttribute {
                at: Locator::Path(vec![]),
                name: "x".into(),
                value: "2".into(),
            }],
        )
        .unwrap();
        let edit = &applied.steps[0][0];
        assert_eq!(edit.from, 15);
        assert_eq!(edit.from16, 13);
    }
}
