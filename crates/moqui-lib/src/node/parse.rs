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

//! Hand-written XML reader for the Moqui corpus. No XML crate.
//!
//! It scans bytes: every delimiter is ASCII, so a multi-byte character is never taken for one. A name, and a value or
//! text with no entity in it, is a slice of the source; only what is kept is copied. The line and column of a node
//! (and of an error) are counted from the last place they were found, so the scan does not count each character.

use std::borrow::Cow;
use std::rc::Rc;

use crate::error::Error;
use crate::node::{intern, Attributes, MNode};

pub(crate) fn parse_document(location: Option<&str>, text: &str, root_only: bool) -> Result<MNode, Error> {
    parse_document_with(location, text, root_only, false)
}

/// [`parse_document`], with `raw_text` keeping each element's text as written, whitespace included.
pub(crate) fn parse_document_with(
    location: Option<&str>,
    text: &str,
    root_only: bool,
    raw_text: bool,
) -> Result<MNode, Error> {
    let mut parser = Parser::new(location, text, raw_text);
    parser.start_document()?;
    let root = parser.parse_element(root_only)?;
    if !root_only {
        parser.end_document()?;
    }
    Ok(root)
}

/// A document of one element with no child elements: its name and attributes, borrowed from the text where they need
/// no decoding. See [`MNode::parse_flat`].
pub struct FlatElement<'a> {
    pub name: &'a str,
    pub attributes: Vec<(&'a str, Cow<'a, str>)>,
}

/// The root of `text` when it has no child elements, checked as [`parse_document`] checks it; `None` when it has some.
pub(crate) fn parse_flat<'a>(location: Option<&str>, text: &'a str) -> Result<Option<FlatElement<'a>>, Error> {
    let mut parser = Parser::new(location, text, true);
    parser.start_document()?;
    let tag = parser.read_start_tag()?;
    if !tag.empty {
        loop {
            if parser.eof() {
                return Err(parser.error(format!("unclosed element <{}>", tag.name)));
            }
            if parser.starts_with("</") {
                parser.read_end_tag(tag.name)?;
                break;
            }
            if parser.starts_with("<!--") {
                parser.skip_comment()?;
            } else if parser.starts_with("<?") {
                parser.skip_pi()?;
            } else if parser.starts_with("<![CDATA[") {
                parser.read_cdata()?;
            } else if parser.starts_with("<!") {
                return Err(parser.declaration_error());
            } else if parser.starts_with("<") {
                return Ok(None);
            } else {
                parser.read_text()?;
            }
        }
    }
    parser.end_document()?;
    Ok(Some(FlatElement {
        name: tag.name,
        attributes: tag.attributes,
    }))
}

struct StartTag<'a> {
    name: &'a str,
    attributes: Vec<(&'a str, Cow<'a, str>)>,
    empty: bool,
}

/// One step of [`XmlEvents`]. Text is raw character data, whitespace included, as SAX
/// `characters` gives it. A CDATA section is its own text event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XmlEvent {
    Start {
        name: String,
        attributes: Vec<(String, String)>,
    },
    Text(String),
    End(String),
}

/// Pull reader over the same rules as the tree parser, for a caller that keeps one record at a
/// time (`FlatXmlExtractor`). An empty element gives a start and then an end.
pub struct XmlEvents<'a> {
    parser: Parser<'a>,
    open: Vec<&'a str>,
    pending_end: Option<&'a str>,
    started: bool,
}

impl<'a> XmlEvents<'a> {
    pub fn new(location: Option<&str>, text: &'a str) -> Self {
        Self {
            parser: Parser::new(location, text, false),
            open: Vec::new(),
            pending_end: None,
            started: false,
        }
    }

    /// The next event, or `None` after the root element has closed.
    pub fn next_event(&mut self) -> Result<Option<XmlEvent>, Error> {
        if let Some(name) = self.pending_end.take() {
            return Ok(Some(XmlEvent::End(name.to_string())));
        }
        if !self.started {
            self.started = true;
            self.parser.start_document()?;
            return self.start();
        }
        let parser = &mut self.parser;
        let Some(top) = self.open.last().copied() else {
            parser.end_document()?;
            return Ok(None);
        };
        loop {
            if parser.eof() {
                return Err(parser.error(format!("unclosed element <{top}>")));
            }
            if parser.starts_with("</") {
                parser.read_end_tag(top)?;
                self.open.pop();
                return Ok(Some(XmlEvent::End(top.to_string())));
            }
            if parser.starts_with("<!--") {
                parser.skip_comment()?;
                continue;
            }
            if parser.starts_with("<?") {
                parser.skip_pi()?;
                continue;
            }
            if parser.starts_with("<![CDATA[") {
                return Ok(Some(XmlEvent::Text(parser.read_cdata()?.to_string())));
            }
            if parser.starts_with("<!") {
                return Err(parser.declaration_error());
            }
            if parser.starts_with("<") {
                return self.start();
            }
            return Ok(Some(XmlEvent::Text(parser.read_text()?.into_owned())));
        }
    }

    fn start(&mut self) -> Result<Option<XmlEvent>, Error> {
        let tag = self.parser.read_start_tag()?;
        if tag.empty {
            self.pending_end = Some(tag.name);
        } else {
            self.open.push(tag.name);
        }
        Ok(Some(XmlEvent::Start {
            name: tag.name.to_string(),
            attributes: tag
                .attributes
                .into_iter()
                .map(|(name, value)| (name.to_string(), value.into_owned()))
                .collect(),
        }))
    }
}

/// Where the line and column were last counted. See [`Parser::position_at`].
#[derive(Clone, Copy)]
struct Mark {
    index: usize,
    line: u32,
    column: u32,
    /// The byte before `index` was a `\r`, so a `\n` there ends no other line.
    after_cr: bool,
}

struct Parser<'a> {
    src: &'a str,
    bytes: &'a [u8],
    index: usize,
    mark: Mark,
    file: Option<Rc<str>>,
    /// Elements open in `parse_element`, bounded by [`MAX_DEPTH`]: nesting recurses.
    depth: usize,
    /// Keep element text as written. Otherwise it is trimmed, and whitespace only is no text, as Java's `MNode`.
    raw_text: bool,
    /// The children of the elements open, each element's after the index it started at, so its list is made once at
    /// its size.
    children: Vec<MNode>,
}

/// Deepest element nesting the tree parser accepts. A deeper document is an error, where recursing on it would
/// overflow the stack, which aborts the process instead of failing the one parse.
const MAX_DEPTH: usize = 256;

impl<'a> Parser<'a> {
    fn new(location: Option<&str>, src: &'a str, raw_text: bool) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            index: 0,
            mark: Mark {
                index: 0,
                line: 1,
                column: 1,
                after_cr: false,
            },
            file: location.filter(|value| !value.is_empty()).map(Rc::from),
            depth: 0,
            raw_text,
            children: Vec::new(),
        }
    }

    /// The prologue, up to the root element's `<`.
    fn start_document(&mut self) -> Result<(), Error> {
        if self.rest().starts_with('\u{feff}') {
            self.index += '\u{feff}'.len_utf8();
        }
        self.skip_misc()?;
        if self.eof() {
            return Err(self.error("document has no root element"));
        }
        if self.starts_with("<!") {
            return Err(self.declaration_error());
        }
        if !self.starts_with("<") || self.starts_with("</") || self.starts_with("<?") {
            return Err(self.error("document has no root element"));
        }
        Ok(())
    }

    /// Only comments, processing instructions and whitespace after the root element.
    fn end_document(&mut self) -> Result<(), Error> {
        self.skip_misc()?;
        if !self.eof() {
            return Err(self.error("trailing content after the root element"));
        }
        Ok(())
    }

    /// The line and column of byte `at`, counted on from the last mark, which only moves forward.
    fn position_at(&mut self, at: usize) -> (u32, u32) {
        let mut mark = self.mark;
        if at > mark.index {
            for &byte in &self.bytes[mark.index..at] {
                match byte {
                    b'\n' => {
                        if !mark.after_cr {
                            mark.line += 1;
                        }
                        mark.column = 1;
                        mark.after_cr = false;
                    }
                    b'\r' => {
                        mark.line += 1;
                        mark.column = 1;
                        mark.after_cr = true;
                    }
                    _ => {
                        // A character's first byte counts; its continuation bytes do not.
                        if byte & 0xC0 != 0x80 {
                            mark.column += 1;
                        }
                        mark.after_cr = false;
                    }
                }
            }
            mark.index = at;
            self.mark = mark;
        }
        (mark.line, mark.column)
    }

    fn error(&mut self, message: impl Into<String>) -> Error {
        self.error_at(message, self.index)
    }

    fn error_at(&mut self, message: impl Into<String>, at: usize) -> Error {
        let (line, column) = self.position_at(at);
        Error::at(message, self.file.as_deref().map(str::to_string), line, column)
    }

    fn eof(&self) -> bool {
        self.index >= self.bytes.len()
    }

    fn rest(&self) -> &'a str {
        &self.src[self.index..]
    }

    fn starts_with(&self, prefix: &str) -> bool {
        self.bytes[self.index..].starts_with(prefix.as_bytes())
    }

    fn peek_char(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn skip_ws(&mut self) {
        while let Some(&byte) = self.bytes.get(self.index) {
            if byte.is_ascii() {
                if !matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C) {
                    break;
                }
                self.index += 1;
            } else {
                match self.peek_char() {
                    Some(ch) if ch.is_whitespace() => self.index += ch.len_utf8(),
                    _ => break,
                }
            }
        }
    }

    fn skip_misc(&mut self) -> Result<(), Error> {
        loop {
            self.skip_ws();
            if self.starts_with("<?") {
                self.skip_pi()?;
            } else if self.starts_with("<!--") {
                self.skip_comment()?;
            } else {
                break;
            }
        }
        Ok(())
    }

    /// Past the end of `marker`, searched from `skip` bytes on; an error at the start when it is missing.
    fn skip_past(&mut self, skip: usize, marker: &str, unclosed: &str) -> Result<&'a str, Error> {
        let start = self.index;
        let from = (start + skip).min(self.bytes.len());
        match self.src[from..].find(marker) {
            Some(offset) => {
                self.index = from + offset + marker.len();
                Ok(&self.src[from..from + offset])
            }
            None => {
                self.index = self.bytes.len();
                Err(self.error_at(unclosed, start))
            }
        }
    }

    fn skip_pi(&mut self) -> Result<(), Error> {
        self.skip_past(2, "?>", "unclosed processing instruction").map(|_| ())
    }

    fn skip_comment(&mut self) -> Result<(), Error> {
        self.skip_past(4, "-->", "unclosed comment").map(|_| ())
    }

    fn read_cdata(&mut self) -> Result<&'a str, Error> {
        self.skip_past("<![CDATA[".len(), "]]>", "unclosed CDATA section")
    }

    fn parse_element(&mut self, root_only: bool) -> Result<MNode, Error> {
        if self.depth >= MAX_DEPTH {
            return Err(self.error(format!("elements nested deeper than {MAX_DEPTH}")));
        }
        self.depth += 1;
        let parsed = self.parse_element_inner(root_only);
        self.depth -= 1;
        parsed
    }

    fn parse_element_inner(&mut self, root_only: bool) -> Result<MNode, Error> {
        let (line, column) = self.position_at(self.index);
        let StartTag {
            name,
            attributes,
            empty,
        } = self.read_start_tag()?;
        let attributes = owned_attributes(attributes);
        if empty || root_only {
            return Ok(MNode::from_parts(
                intern(name),
                attributes,
                None,
                Vec::new(),
                self.file.clone(),
                line,
                column,
            ));
        }
        let first_child = self.children.len();
        let mut text: Option<Cow<'a, str>> = None;
        loop {
            if self.eof() {
                return Err(self.error(format!("unclosed element <{name}>")));
            }
            if self.starts_with("</") {
                self.read_end_tag(name)?;
                break;
            }
            if self.starts_with("<!--") {
                self.skip_comment()?;
                continue;
            }
            if self.starts_with("<?") {
                self.skip_pi()?;
                continue;
            }
            if self.starts_with("<![CDATA[") {
                let cdata = self.read_cdata()?;
                self.add_text(&mut text, Cow::Borrowed(cdata));
                continue;
            }
            if self.starts_with("<!") {
                return Err(self.declaration_error());
            }
            if self.starts_with("<") {
                let child = self.parse_element(false)?;
                self.children.push(child);
                continue;
            }
            let piece = self.read_text()?;
            self.add_text(&mut text, piece);
        }
        let text = if self.raw_text {
            text.filter(|text| !text.is_empty()).map(Box::from)
        } else {
            text.as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(Box::from)
        };
        let children: Vec<MNode> = self.children.drain(first_child..).collect();
        Ok(MNode::from_parts(
            intern(name),
            attributes,
            text,
            children,
            self.file.clone(),
            line,
            column,
        ))
    }

    /// Text is the element's character data in order. Trimmed, whitespace before any other text is no text, so it is
    /// not kept.
    fn add_text(&self, text: &mut Option<Cow<'a, str>>, piece: Cow<'a, str>) {
        if piece.is_empty() {
            return;
        }
        match text {
            None if !self.raw_text && piece.trim().is_empty() => {}
            None => *text = Some(piece),
            Some(existing) => existing.to_mut().push_str(&piece),
        }
    }

    /// `<name attr="value" ...>` or `<name .../>`. The cursor is on the `<`.
    fn read_start_tag(&mut self) -> Result<StartTag<'a>, Error> {
        if !self.starts_with("<") {
            return Err(self.error("expected an element"));
        }
        self.index += 1;
        if self.starts_with("!") || self.starts_with("?") || self.starts_with("/") {
            return Err(self.error("expected an element name"));
        }
        let name = self.read_name()?;
        let mut attributes: Vec<(&'a str, Cow<'a, str>)> = Vec::new();
        loop {
            self.skip_ws();
            if self.starts_with("/>") {
                self.index += 2;
                return Ok(StartTag {
                    name,
                    attributes,
                    empty: true,
                });
            }
            if self.starts_with(">") {
                self.index += 1;
                return Ok(StartTag {
                    name,
                    attributes,
                    empty: false,
                });
            }
            if self.eof() {
                return Err(self.error(format!("unclosed start tag <{name}>")));
            }
            let attr_name = self.read_name()?;
            self.skip_ws();
            if !self.starts_with("=") {
                return Err(self.error(format!("attribute {attr_name} has no value")));
            }
            self.index += 1;
            self.skip_ws();
            let attr_value = self.read_attr_value()?;
            // A repeated attribute takes the later value, in the first one's place.
            match attributes.iter_mut().find(|(name, _)| *name == attr_name) {
                Some(slot) => slot.1 = attr_value,
                None => attributes.push((attr_name, attr_value)),
            }
        }
    }

    /// `</name>` for the open element `name`. The cursor is on the `<`.
    fn read_end_tag(&mut self, name: &str) -> Result<(), Error> {
        let start = self.index;
        self.index += 2;
        let end_name = self.read_name()?;
        self.skip_ws();
        if !self.starts_with(">") {
            return Err(self.error(format!("unclosed end tag </{end_name}")));
        }
        self.index += 1;
        if end_name != name {
            return Err(self.error_at(format!("end tag </{end_name}> does not match <{name}>"), start));
        }
        Ok(())
    }

    fn declaration_error(&mut self) -> Error {
        let head: String = self.rest().chars().take(12).collect();
        let upper = head.to_ascii_uppercase();
        let message = if upper.starts_with("<!DOCTYPE") {
            "DOCTYPE is not allowed"
        } else if upper.starts_with("<!ENTITY") {
            "external entity is not allowed"
        } else {
            "this declaration is not allowed"
        };
        self.error(message)
    }

    fn read_name(&mut self) -> Result<&'a str, Error> {
        let start = self.index;
        let ch = self.peek_char().ok_or_else(|| self.error("expected a name"))?;
        if !is_name_start(ch) {
            return Err(self.error(format!("invalid name start '{ch}'")));
        }
        self.index += ch.len_utf8();
        while let Some(&byte) = self.bytes.get(self.index) {
            if byte.is_ascii() {
                if !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'-' | b'.')) {
                    break;
                }
                self.index += 1;
            } else {
                match self.peek_char() {
                    Some(ch) if is_name_char(ch) => self.index += ch.len_utf8(),
                    _ => break,
                }
            }
        }
        Ok(&self.src[start..self.index])
    }

    fn read_attr_value(&mut self) -> Result<Cow<'a, str>, Error> {
        let quote = match self.bytes.get(self.index) {
            None => return Err(self.error("attribute value is missing")),
            Some(&byte @ (b'"' | b'\'')) => byte,
            Some(_) => return Err(self.error("attribute value must be quoted")),
        };
        self.index += 1;
        let mut value: Option<String> = None;
        let mut piece = self.index;
        loop {
            let Some(&byte) = self.bytes.get(self.index) else {
                return Err(self.error("unclosed attribute value"));
            };
            if byte == quote {
                let tail = &self.src[piece..self.index];
                self.index += 1;
                return Ok(match value {
                    None => Cow::Borrowed(tail),
                    Some(mut value) => {
                        value.push_str(tail);
                        Cow::Owned(value)
                    }
                });
            }
            match byte {
                b'<' => return Err(self.error("'<' is not allowed in an attribute value")),
                b'&' => {
                    let decoded = value.get_or_insert_with(String::new);
                    decoded.push_str(&self.src[piece..self.index]);
                    let ch = self.read_entity()?;
                    value.get_or_insert_with(String::new).push(ch);
                    piece = self.index;
                }
                _ => self.index += 1,
            }
        }
    }

    fn read_text(&mut self) -> Result<Cow<'a, str>, Error> {
        let mut text: Option<String> = None;
        let mut piece = self.index;
        while let Some(&byte) = self.bytes.get(self.index) {
            match byte {
                b'<' => break,
                b']' if self.starts_with("]]>") => return Err(self.error("']]>' is not allowed in element text")),
                b'&' => {
                    let decoded = text.get_or_insert_with(String::new);
                    decoded.push_str(&self.src[piece..self.index]);
                    let ch = self.read_entity()?;
                    text.get_or_insert_with(String::new).push(ch);
                    piece = self.index;
                }
                _ => self.index += 1,
            }
        }
        let tail = &self.src[piece..self.index];
        Ok(match text {
            None => Cow::Borrowed(tail),
            Some(mut text) => {
                text.push_str(tail);
                Cow::Owned(text)
            }
        })
    }

    /// `&name;` or a character reference. The cursor is on the `&`.
    fn read_entity(&mut self) -> Result<char, Error> {
        let start = self.index;
        self.index += 1;
        let body_start = self.index;
        while let Some(ch) = self.peek_char() {
            if ch == ';' || ch.is_whitespace() || ch == '<' || ch == '&' {
                break;
            }
            self.index += ch.len_utf8();
            if self.index - body_start > 16 {
                break;
            }
        }
        let body = &self.src[body_start..self.index];
        if !self.starts_with(";") {
            return Err(self.error_at(format!("unterminated entity '&{body}'"), start));
        }
        self.index += 1;
        let Some(ch) = decode_entity(body) else {
            return Err(self.error_at(format!("unknown entity '&{body};'"), start));
        };
        if !is_xml_char(ch) {
            return Err(self.error_at(format!("entity '&{body};' is not an XML character"), start));
        }
        Ok(ch)
    }
}

/// Attribute names interned, values copied at their size.
fn owned_attributes(attributes: Vec<(&str, Cow<'_, str>)>) -> Attributes {
    attributes
        .into_iter()
        .map(|(name, value)| (intern(name), Box::from(value)))
        .collect()
}

fn decode_entity(body: &str) -> Option<char> {
    match body {
        "lt" => Some('<'),
        "gt" => Some('>'),
        "amp" => Some('&'),
        "apos" => Some('\''),
        "quot" => Some('"'),
        _ if body.starts_with("#x") || body.starts_with("#X") => {
            let hex = &body[2..];
            if hex.is_empty() || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return None;
            }
            let code = u32::from_str_radix(hex, 16).ok()?;
            char::from_u32(code)
        }
        _ if body.starts_with('#') => {
            let digits = &body[1..];
            if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
                return None;
            }
            let code = digits.parse::<u32>().ok()?;
            char::from_u32(code)
        }
        _ => None,
    }
}

fn is_xml_char(ch: char) -> bool {
    matches!(ch, '\u{9}' | '\u{A}' | '\u{D}' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

fn is_name_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_' || ch == ':' || (!ch.is_ascii() && ch.is_alphabetic())
}

fn is_name_char(ch: char) -> bool {
    is_name_start(ch) || ch.is_ascii_digit() || ch == '-' || ch == '.' || (!ch.is_ascii() && ch.is_numeric())
}
