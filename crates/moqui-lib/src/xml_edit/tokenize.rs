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

//! A byte-exact XML scanner: every byte of the text belongs to exactly one node's span (text, comment, CDATA, processing
//! instruction, doctype, or an element with its start tag, content and end tag), so an edit can replace a span and leave
//! the rest of the file as it was. It reads what Moqui files are; it does not validate, expand entities or resolve
//! namespaces.

use std::ops::Range;

use crate::error::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The document: the virtual parent of the prolog, the root element and what follows it.
    Document,
    Element,
    Text,
    Comment,
    CData,
    Pi,
    Doctype,
}

#[derive(Clone, Debug)]
pub struct Attr {
    pub name: Range<usize>,
    /// The value between the quotes.
    pub value: Range<usize>,
    pub quote: u8,
    /// From the attribute's name through its closing quote.
    pub span: Range<usize>,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub kind: Kind,
    /// The whole node: for an element from its `<` through its end tag's `>` (or its `/>`).
    pub span: Range<usize>,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// An element's name.
    pub name: Range<usize>,
    pub attrs: Vec<Attr>,
    /// An element's start tag, `<` through `>` or `/>`.
    pub start_tag: Range<usize>,
    pub self_closing: bool,
    /// Between the start tag and the end tag; empty at the start tag's end when self-closing.
    pub content: Range<usize>,
    /// The end tag, empty when self-closing.
    pub end_tag: Range<usize>,
}

impl Node {
    fn new(kind: Kind, start: usize, parent: Option<usize>) -> Self {
        Self {
            kind,
            span: start..start,
            parent,
            children: Vec::new(),
            name: start..start,
            attrs: Vec::new(),
            start_tag: start..start,
            self_closing: false,
            content: start..start,
            end_tag: start..start,
        }
    }
}

/// The nodes of a text, the document first.
#[derive(Clone, Debug)]
pub struct Doc<'a> {
    pub text: &'a str,
    pub nodes: Vec<Node>,
}

impl<'a> Doc<'a> {
    /// Scan `text`.
    ///
    /// # Errors
    ///
    /// Returns an error with the line and column of a construct that is not closed, an end tag that does not match, or
    /// text after the content that is not XML.
    pub fn parse(text: &'a str) -> Result<Self, Error> {
        Scanner {
            text,
            bytes: text.as_bytes(),
            at: 0,
            nodes: vec![Node::new(Kind::Document, 0, None)],
        }
        .run()
    }

    #[must_use]
    pub fn slice(&self, range: &Range<usize>) -> &'a str {
        &self.text[range.clone()]
    }

    /// The name of element `index`.
    #[must_use]
    pub fn name(&self, index: usize) -> &'a str {
        self.slice(&self.nodes[index].name)
    }

    /// The element children of `index`, in order.
    pub fn elements(&self, index: usize) -> impl Iterator<Item = usize> + '_ {
        self.nodes[index]
            .children
            .iter()
            .copied()
            .filter(|child| self.nodes[*child].kind == Kind::Element)
    }

    /// The root element.
    #[must_use]
    pub fn root(&self) -> Option<usize> {
        self.elements(0).next()
    }

    /// The value of attribute `name` of element `index`, as written (entities not expanded).
    #[must_use]
    pub fn attr(&self, index: usize, name: &str) -> Option<&'a str> {
        self.nodes[index]
            .attrs
            .iter()
            .find(|attr| self.slice(&attr.name) == name)
            .map(|attr| self.slice(&attr.value))
    }

    /// 1-based line and column of byte `at`.
    #[must_use]
    pub fn position(&self, at: usize) -> (usize, usize) {
        position(self.text, at)
    }
}

pub(crate) fn position(text: &str, at: usize) -> (usize, usize) {
    let before = &text[..at.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |last| last.chars().count()) + 1;
    (line, column)
}

struct Scanner<'a> {
    text: &'a str,
    bytes: &'a [u8],
    at: usize,
    nodes: Vec<Node>,
}

impl<'a> Scanner<'a> {
    fn error(&self, at: usize, message: &str) -> Error {
        let (line, column) = position(self.text, at);
        Error::new(format!("{message} at line {line}, column {column}"))
    }

    fn starts(&self, prefix: &str) -> bool {
        self.bytes[self.at..].starts_with(prefix.as_bytes())
    }

    fn find(&self, from: usize, needle: &str) -> Option<usize> {
        self.text[from..].find(needle).map(|found| from + found)
    }

    fn push(&mut self, node: Node) -> usize {
        let index = self.nodes.len();
        if let Some(parent) = node.parent {
            self.nodes[parent].children.push(index);
        }
        self.nodes.push(node);
        index
    }

    fn run(mut self) -> Result<Doc<'a>, Error> {
        let mut open = 0; // the element (or the document) whose content is being read
        while self.at < self.bytes.len() {
            let start = self.at;
            if self.bytes[start] != b'<' {
                let end = self.find(start, "<").unwrap_or(self.bytes.len());
                let mut node = Node::new(Kind::Text, start, Some(open));
                node.span = start..end;
                self.push(node);
                self.at = end;
                continue;
            }
            if self.starts("<!--") {
                let end = self
                    .find(start + 4, "-->")
                    .ok_or_else(|| self.error(start, "comment not closed"))?
                    + 3;
                self.leaf(Kind::Comment, start, end, open);
            } else if self.starts("<![CDATA[") {
                let end = self
                    .find(start + 9, "]]>")
                    .ok_or_else(|| self.error(start, "CDATA not closed"))?
                    + 3;
                self.leaf(Kind::CData, start, end, open);
            } else if self.starts("<?") {
                let end = self
                    .find(start + 2, "?>")
                    .ok_or_else(|| self.error(start, "processing instruction not closed"))?
                    + 2;
                self.leaf(Kind::Pi, start, end, open);
            } else if self.starts("<!") {
                let end = self.doctype_end(start)?;
                self.leaf(Kind::Doctype, start, end, open);
            } else if self.starts("</") {
                let name_start = start + 2;
                let name_end = self.name_end(name_start);
                let close = self
                    .find(name_end, ">")
                    .ok_or_else(|| self.error(start, "end tag not closed"))?;
                if open == 0 {
                    return Err(self.error(start, "end tag with no element open"));
                }
                let expected = self.nodes[open].name.clone();
                if self.text[name_start..name_end] != self.text[expected] {
                    return Err(self.error(start, "end tag does not match its start tag"));
                }
                let node = &mut self.nodes[open];
                node.content = node.start_tag.end..start;
                node.end_tag = start..close + 1;
                node.span = node.start_tag.start..close + 1;
                open = node.parent.unwrap_or(0);
                self.at = close + 1;
            } else {
                let index = self.start_tag(start, open)?;
                if !self.nodes[index].self_closing {
                    open = index;
                }
            }
        }
        if open != 0 {
            let start = self.nodes[open].span.start;
            return Err(self.error(start, "element not closed"));
        }
        self.nodes[0].span = 0..self.bytes.len();
        self.nodes[0].content = 0..self.bytes.len();
        Ok(Doc {
            text: self.text,
            nodes: self.nodes,
        })
    }

    fn leaf(&mut self, kind: Kind, start: usize, end: usize, parent: usize) {
        let mut node = Node::new(kind, start, Some(parent));
        node.span = start..end;
        self.push(node);
        self.at = end;
    }

    fn doctype_end(&self, start: usize) -> Result<usize, Error> {
        let mut depth = 0_i32;
        for (offset, byte) in self.bytes[start..].iter().enumerate() {
            match byte {
                b'[' => depth += 1,
                b']' => depth -= 1,
                b'>' if depth <= 0 => return Ok(start + offset + 1),
                _ => {}
            }
        }
        Err(self.error(start, "declaration not closed"))
    }

    fn name_end(&self, from: usize) -> usize {
        let mut end = from;
        while end < self.bytes.len() {
            let byte = self.bytes[end];
            if byte.is_ascii_whitespace() || matches!(byte, b'>' | b'/' | b'=' | b'<' | b'"' | b'\'') {
                break;
            }
            end += 1;
        }
        end
    }

    fn skip_space(&mut self) {
        while self.at < self.bytes.len() && self.bytes[self.at].is_ascii_whitespace() {
            self.at += 1;
        }
    }

    fn start_tag(&mut self, start: usize, parent: usize) -> Result<usize, Error> {
        let mut node = Node::new(Kind::Element, start, Some(parent));
        let name_end = self.name_end(start + 1);
        if name_end == start + 1 {
            return Err(self.error(start, "a < that starts no tag"));
        }
        node.name = start + 1..name_end;
        self.at = name_end;
        loop {
            self.skip_space();
            if self.at >= self.bytes.len() {
                return Err(self.error(start, "start tag not closed"));
            }
            if self.starts("/>") {
                node.self_closing = true;
                self.at += 2;
                break;
            }
            if self.bytes[self.at] == b'>' {
                self.at += 1;
                break;
            }
            let name_start = self.at;
            let name_end = self.name_end(name_start);
            if name_end == name_start {
                return Err(self.error(self.at, "an attribute has no name"));
            }
            self.at = name_end;
            self.skip_space();
            if self.at >= self.bytes.len() || self.bytes[self.at] != b'=' {
                return Err(self.error(name_start, "an attribute has no value"));
            }
            self.at += 1;
            self.skip_space();
            let quote = *self.bytes.get(self.at).unwrap_or(&0);
            if quote != b'"' && quote != b'\'' {
                return Err(self.error(self.at, "an attribute value is not quoted"));
            }
            let value_start = self.at + 1;
            let value_end = self.bytes[value_start..]
                .iter()
                .position(|byte| *byte == quote)
                .map(|offset| value_start + offset)
                .ok_or_else(|| self.error(name_start, "an attribute value is not closed"))?;
            self.at = value_end + 1;
            node.attrs.push(Attr {
                name: name_start..name_end,
                value: value_start..value_end,
                quote,
                span: name_start..self.at,
            });
        }
        node.start_tag = start..self.at;
        node.content = self.at..self.at;
        node.end_tag = self.at..self.at;
        node.span = start..self.at;
        Ok(self.push(node))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every byte belongs to one node: the document's children tile the text, and each element's children tile its
    /// content.
    pub(crate) fn assert_tiles(doc: &Doc<'_>, label: &str) {
        fn tile(doc: &Doc<'_>, index: usize, label: &str) {
            let node = &doc.nodes[index];
            let mut at = node.content.start;
            for child in &node.children {
                let span = &doc.nodes[*child].span;
                assert_eq!(
                    span.start,
                    at,
                    "{label}: a gap or overlap before node {child} at {:?}",
                    doc.position(at)
                );
                at = span.end;
                if doc.nodes[*child].kind == Kind::Element {
                    tile(doc, *child, label);
                }
            }
            assert_eq!(at, node.content.end, "{label}: a gap at the end of node {index}");
        }
        tile(doc, 0, label);
    }

    #[test]
    fn spans_of_each_kind() {
        let text = "<?xml version=\"1.0\"?>\n<!-- c -->\n<a x='1' y = \"2\"><b/>t<![CDATA[<x>]]></a>\n";
        let doc = Doc::parse(text).unwrap();
        assert_tiles(&doc, "spans");
        let root = doc.root().unwrap();
        assert_eq!(doc.name(root), "a");
        assert_eq!(doc.attr(root, "y"), Some("2"));
        assert_eq!(doc.nodes[root].attrs[0].quote, b'\'');
        let kinds: Vec<Kind> = doc.nodes[root]
            .children
            .iter()
            .map(|child| doc.nodes[*child].kind)
            .collect();
        assert_eq!(kinds, [Kind::Element, Kind::Text, Kind::CData]);
        assert!(doc.nodes[doc.nodes[root].children[0]].self_closing);
    }

    #[test]
    fn errors_say_where() {
        let err = Doc::parse("<a>\n  <b>\n</a>").unwrap_err().to_string();
        assert!(err.contains("line 3, column 1"), "{err}");
        assert!(Doc::parse("<a x=1/>").is_err());
        assert!(Doc::parse("<a>").is_err());
    }

    /// Every XML file of the framework and the components scans with no gaps or overlaps. Not the component template,
    /// whose placeholders (`<@package@.@Class@Item>`) are not XML names.
    #[test]
    fn the_corpus_scans_byte_exact() {
        let runtime = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime");
        let template = runtime.join("dev-component/template");
        let mut files = Vec::new();
        let mut stack = vec![
            runtime.join("base-component"),
            runtime.join("component"),
            runtime.join("dev-component"),
        ];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if path.is_dir() {
                    if !name.starts_with('.') && name != "target" && name != "node_modules" && path != template {
                        stack.push(path);
                    }
                } else if name.ends_with(".xml") || name.ends_with(".xsd") {
                    files.push(path);
                }
            }
        }
        assert!(files.len() > 500, "{}", files.len());
        let mut scanned = 0;
        for file in &files {
            let Ok(text) = std::fs::read_to_string(file) else {
                continue;
            };
            match Doc::parse(&text) {
                Ok(doc) => {
                    assert_tiles(&doc, &file.display().to_string());
                    scanned += 1;
                }
                Err(err) => panic!("{}: {err}", file.display()),
            }
        }
        assert_eq!(scanned, files.len());
    }
}
