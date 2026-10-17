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

use std::cell::RefCell;
use std::rc::Weak;

use indexmap::IndexMap;

use crate::error::Error;
use crate::node::{MNode, NodeInner};

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
    let mut parser = Parser {
        src: text,
        index: 0,
        line: 1,
        column: 1,
        file: location.filter(|value| !value.is_empty()).map(str::to_string),
        depth: 0,
        raw_text,
    };
    parser.skip_bom();
    parser.skip_misc()?;
    if parser.eof() {
        return Err(parser.error("document has no root element"));
    }
    if parser.starts_with("<!") {
        return Err(parser.declaration_error());
    }
    if !parser.starts_with("<") || parser.starts_with("</") || parser.starts_with("<?") {
        return Err(parser.error("document has no root element"));
    }
    let root = parser.parse_element(None, root_only)?;
    if !root_only {
        parser.skip_misc()?;
        if !parser.eof() {
            return Err(parser.error("trailing content after the root element"));
        }
    }
    Ok(root)
}

struct StartTag {
    name: String,
    attributes: IndexMap<String, String>,
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
    open: Vec<String>,
    pending_end: Option<String>,
    started: bool,
}

impl<'a> XmlEvents<'a> {
    pub fn new(location: Option<&str>, text: &'a str) -> Self {
        Self {
            parser: Parser {
                src: text,
                index: 0,
                line: 1,
                column: 1,
                file: location.filter(|value| !value.is_empty()).map(str::to_string),
                depth: 0,
                raw_text: false,
            },
            open: Vec::new(),
            pending_end: None,
            started: false,
        }
    }

    /// The next event, or `None` after the root element has closed.
    pub fn next_event(&mut self) -> Result<Option<XmlEvent>, Error> {
        if let Some(name) = self.pending_end.take() {
            return Ok(Some(XmlEvent::End(name)));
        }
        let parser = &mut self.parser;
        if !self.started {
            self.started = true;
            parser.skip_bom();
            parser.skip_misc()?;
            if parser.eof() {
                return Err(parser.error("document has no root element"));
            }
            if parser.starts_with("<!") {
                return Err(parser.declaration_error());
            }
            if !parser.starts_with("<") || parser.starts_with("</") || parser.starts_with("<?") {
                return Err(parser.error("document has no root element"));
            }
            return self.start();
        }
        let Some(top) = self.open.last().cloned() else {
            parser.skip_misc()?;
            if !parser.eof() {
                return Err(parser.error("trailing content after the root element"));
            }
            return Ok(None);
        };
        loop {
            if parser.eof() {
                return Err(parser.error(format!("unclosed element <{top}>")));
            }
            if parser.starts_with("</") {
                parser.read_end_tag(&top)?;
                self.open.pop();
                return Ok(Some(XmlEvent::End(top)));
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
                return Ok(Some(XmlEvent::Text(parser.read_cdata()?)));
            }
            if parser.starts_with("<!") {
                return Err(parser.declaration_error());
            }
            if parser.starts_with("<") {
                return self.start();
            }
            return Ok(Some(XmlEvent::Text(parser.read_text()?)));
        }
    }

    fn start(&mut self) -> Result<Option<XmlEvent>, Error> {
        let tag = self.parser.read_start_tag()?;
        if tag.empty {
            self.pending_end = Some(tag.name.clone());
        } else {
            self.open.push(tag.name.clone());
        }
        Ok(Some(XmlEvent::Start {
            name: tag.name,
            attributes: tag.attributes.into_iter().collect(),
        }))
    }
}

struct Parser<'a> {
    src: &'a str,
    index: usize,
    line: u32,
    column: u32,
    file: Option<String>,
    /// Elements open in `parse_element`, bounded by [`MAX_DEPTH`]: nesting recurses.
    depth: usize,
    /// Keep element text as written. Otherwise it is trimmed, and whitespace only is no text, as Java's `MNode`.
    raw_text: bool,
}

/// Deepest element nesting the tree parser accepts. A deeper document is an error, where recursing on it would
/// overflow the stack, which aborts the process instead of failing the one parse.
const MAX_DEPTH: usize = 256;

impl<'a> Parser<'a> {
    fn error(&self, message: impl Into<String>) -> Error {
        Error::at(message, self.file.clone(), self.line, self.column)
    }

    fn eof(&self) -> bool {
        self.index >= self.src.len()
    }

    fn rest(&self) -> &'a str {
        &self.src[self.index..]
    }

    fn starts_with(&self, prefix: &str) -> bool {
        self.rest().starts_with(prefix)
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.index += ch.len_utf8();
        if ch == '\r' {
            if self.rest().starts_with('\n') {
                self.index += 1;
            }
            self.line += 1;
            self.column = 1;
        } else if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(ch)
    }

    fn skip_bom(&mut self) {
        if self.rest().starts_with('\u{feff}') {
            self.bump();
        }
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(ch) if ch.is_whitespace()) {
            self.bump();
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

    fn skip_pi(&mut self) -> Result<(), Error> {
        let start_line = self.line;
        let start_col = self.column;
        self.bump();
        self.bump();
        while !self.eof() && !self.starts_with("?>") {
            self.bump();
        }
        if !self.starts_with("?>") {
            return Err(Error::at(
                "unclosed processing instruction",
                self.file.clone(),
                start_line,
                start_col,
            ));
        }
        self.bump();
        self.bump();
        Ok(())
    }

    fn skip_comment(&mut self) -> Result<(), Error> {
        let start_line = self.line;
        let start_col = self.column;
        for _ in 0..4 {
            self.bump();
        }
        while !self.eof() && !self.starts_with("-->") {
            self.bump();
        }
        if !self.starts_with("-->") {
            return Err(Error::at("unclosed comment", self.file.clone(), start_line, start_col));
        }
        self.bump();
        self.bump();
        self.bump();
        Ok(())
    }

    fn parse_element(&mut self, parent: Option<Weak<RefCell<NodeInner>>>, root_only: bool) -> Result<MNode, Error> {
        if self.depth >= MAX_DEPTH {
            return Err(self.error(format!("elements nested deeper than {MAX_DEPTH}")));
        }
        self.depth += 1;
        let parsed = self.parse_element_inner(parent, root_only);
        self.depth -= 1;
        parsed
    }

    fn parse_element_inner(
        &mut self,
        parent: Option<Weak<RefCell<NodeInner>>>,
        root_only: bool,
    ) -> Result<MNode, Error> {
        let line = self.line;
        let column = self.column;
        let StartTag {
            name,
            attributes,
            empty,
        } = self.read_start_tag()?;
        if empty || root_only {
            return Ok(MNode::from_parts(
                name,
                attributes,
                None,
                Vec::new(),
                parent,
                self.file.clone(),
                line,
                column,
            ));
        }
        let mut text = String::new();
        let mut children = Vec::new();
        loop {
            if self.eof() {
                return Err(self.error(format!("unclosed element <{name}>")));
            }
            if self.starts_with("</") {
                self.read_end_tag(&name)?;
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
                text.push_str(&self.read_cdata()?);
                continue;
            }
            if self.starts_with("<!") {
                return Err(self.declaration_error());
            }
            if self.starts_with("<") {
                children.push(self.parse_element(None, false)?);
                continue;
            }
            text.push_str(&self.read_text()?);
        }
        let text = if self.raw_text {
            Some(text).filter(|text| !text.is_empty())
        } else {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        };
        Ok(MNode::from_parts(
            name,
            attributes,
            text,
            children,
            parent,
            self.file.clone(),
            line,
            column,
        ))
    }

    /// `<name attr="value" ...>` or `<name .../>`. The cursor is on the `<`.
    fn read_start_tag(&mut self) -> Result<StartTag, Error> {
        if !self.starts_with("<") {
            return Err(self.error("expected an element"));
        }
        self.bump();
        if self.starts_with("!") || self.starts_with("?") || self.starts_with("/") {
            return Err(self.error("expected an element name"));
        }
        let name = self.read_name()?;
        let mut attributes = IndexMap::new();
        loop {
            self.skip_ws();
            if self.starts_with("/>") {
                self.bump();
                self.bump();
                return Ok(StartTag {
                    name,
                    attributes,
                    empty: true,
                });
            }
            if self.starts_with(">") {
                self.bump();
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
            self.bump();
            self.skip_ws();
            let attr_value = self.read_attr_value()?;
            attributes.insert(attr_name, attr_value);
        }
    }

    /// `</name>` for the open element `name`. The cursor is on the `<`.
    fn read_end_tag(&mut self, name: &str) -> Result<(), Error> {
        let end_line = self.line;
        let end_col = self.column;
        self.bump();
        self.bump();
        let end_name = self.read_name()?;
        self.skip_ws();
        if !self.starts_with(">") {
            return Err(self.error(format!("unclosed end tag </{end_name}")));
        }
        self.bump();
        if end_name != name {
            return Err(Error::at(
                format!("end tag </{end_name}> does not match <{name}>"),
                self.file.clone(),
                end_line,
                end_col,
            ));
        }
        Ok(())
    }

    fn declaration_error(&mut self) -> Error {
        let line = self.line;
        let column = self.column;
        let rest = self.rest();
        let head: String = rest.chars().take(12).collect();
        let upper = head.to_ascii_uppercase();
        let message = if upper.starts_with("<!DOCTYPE") {
            "DOCTYPE is not allowed"
        } else if upper.starts_with("<!ENTITY") {
            "external entity is not allowed"
        } else {
            "this declaration is not allowed"
        };
        Error::at(message, self.file.clone(), line, column)
    }

    fn read_name(&mut self) -> Result<String, Error> {
        let ch = self.peek().ok_or_else(|| self.error("expected a name"))?;
        if !is_name_start(ch) {
            return Err(self.error(format!("invalid name start '{ch}'")));
        }
        let mut name = String::new();
        while let Some(ch) = self.peek() {
            if !is_name_char(ch) {
                break;
            }
            name.push(ch);
            self.bump();
        }
        Ok(name)
    }

    fn read_attr_value(&mut self) -> Result<String, Error> {
        let quote = self.peek().ok_or_else(|| self.error("attribute value is missing"))?;
        if quote != '"' && quote != '\'' {
            return Err(self.error("attribute value must be quoted"));
        }
        self.bump();
        let mut value = String::new();
        loop {
            let ch = self.peek().ok_or_else(|| self.error("unclosed attribute value"))?;
            if ch == quote {
                self.bump();
                break;
            }
            if ch == '<' {
                return Err(self.error("'<' is not allowed in an attribute value"));
            }
            if ch == '&' {
                value.push(self.read_entity()?);
            } else {
                value.push(ch);
                self.bump();
            }
        }
        Ok(value)
    }

    fn read_text(&mut self) -> Result<String, Error> {
        let mut text = String::new();
        while let Some(ch) = self.peek() {
            if ch == '<' {
                break;
            }
            if self.starts_with("]]>") {
                return Err(self.error("']]>' is not allowed in element text"));
            }
            if ch == '&' {
                text.push(self.read_entity()?);
            } else {
                text.push(ch);
                self.bump();
            }
        }
        Ok(text)
    }

    fn read_cdata(&mut self) -> Result<String, Error> {
        let start_line = self.line;
        let start_col = self.column;
        for _ in 0.."<![CDATA[".len() {
            self.bump();
        }
        let mut text = String::new();
        while !self.eof() && !self.starts_with("]]>") {
            if let Some(ch) = self.bump() {
                text.push(ch);
            }
        }
        if !self.starts_with("]]>") {
            return Err(Error::at(
                "unclosed CDATA section",
                self.file.clone(),
                start_line,
                start_col,
            ));
        }
        self.bump();
        self.bump();
        self.bump();
        Ok(text)
    }

    fn read_entity(&mut self) -> Result<char, Error> {
        let line = self.line;
        let column = self.column;
        self.bump();
        let mut body = String::new();
        while let Some(ch) = self.peek() {
            if ch == ';' {
                break;
            }
            if ch.is_whitespace() || ch == '<' || ch == '&' {
                break;
            }
            body.push(ch);
            self.bump();
            if body.len() > 16 {
                break;
            }
        }
        if !self.starts_with(";") {
            return Err(Error::at(
                format!("unterminated entity '&{body}'"),
                self.file.clone(),
                line,
                column,
            ));
        }
        self.bump();
        let ch = decode_entity(&body)
            .ok_or_else(|| Error::at(format!("unknown entity '&{body};'"), self.file.clone(), line, column))?;
        if !is_xml_char(ch) {
            return Err(Error::at(
                format!("entity '&{body};' is not an XML character"),
                self.file.clone(),
                line,
                column,
            ));
        }
        Ok(ch)
    }
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
