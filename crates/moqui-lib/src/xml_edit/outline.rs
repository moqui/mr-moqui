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

//! The outline of an XML text: its elements as a tree, each with the [`Locator::Path`](super::Locator) the operations
//! take, its attributes as values (entities expanded), its line and its span in UTF-16 code units, as a browser editor
//! counts. The visual views of the dev-aide read it (plans/design-dev-aide.md 5.2), so the paths they write are the
//! ones this layer reads.

use serde::Serialize;

use super::tokenize::{Doc, Kind};
use crate::error::Error;

/// One element of the outline.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OutlineNode {
    /// The element child indexes from the root element; `[]` is the root.
    pub path: Vec<usize>,
    pub name: String,
    /// The attributes in their order, each `[name, value]` with entities expanded.
    pub attrs: Vec<(String, String)>,
    /// The 1-based line and column of its `<`.
    pub line: usize,
    pub column: usize,
    /// The element from its `<` through its end, in UTF-16 code units.
    pub from16: usize,
    pub to16: usize,
    /// The text of an element with no element children (a CDATA section's content as is), trimmed; absent when empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<OutlineNode>,
}

impl OutlineNode {
    /// The value of attribute `name`.
    #[must_use]
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// The outline of `text`, from its root element.
///
/// # Errors
///
/// Returns the parse error of a text that is not well formed, or an error when it has no root element.
pub fn outline(text: &str) -> Result<OutlineNode, Error> {
    let doc = Doc::parse(text)?;
    let root = doc.root().ok_or_else(|| Error::new("the text has no root element"))?;
    let offsets = Offsets::of(text);
    Ok(node_of(&doc, &offsets, root, Vec::new()))
}

fn node_of(doc: &Doc<'_>, offsets: &Offsets, index: usize, path: Vec<usize>) -> OutlineNode {
    let node = &doc.nodes[index];
    let children: Vec<usize> = doc.elements(index).collect();
    let text = if children.is_empty() && !node.self_closing {
        let mut text = String::new();
        for child in &node.children {
            let child = &doc.nodes[*child];
            match child.kind {
                Kind::Text => text.push_str(&unescape(doc.slice(&child.span))),
                Kind::CData => {
                    let span = &child.span;
                    text.push_str(&doc.text[span.start + 9..span.end - 3]);
                }
                _ => {}
            }
        }
        let trimmed = text.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    } else {
        None
    };
    let (line, column) = offsets.line_column(doc.text, node.span.start);
    OutlineNode {
        name: doc.name(index).to_string(),
        attrs: node
            .attrs
            .iter()
            .map(|attr| (doc.slice(&attr.name).to_string(), unescape(doc.slice(&attr.value))))
            .collect(),
        line,
        column,
        from16: offsets.utf16(node.span.start),
        to16: offsets.utf16(node.span.end),
        text,
        children: children
            .into_iter()
            .enumerate()
            .map(|(position, child)| {
                let mut child_path = path.clone();
                child_path.push(position);
                node_of(doc, offsets, child, child_path)
            })
            .collect(),
        path,
    }
}

/// Byte offsets to lines and UTF-16 offsets, read once for the text.
struct Offsets {
    line_starts: Vec<usize>,
    /// The UTF-16 offset of each char boundary, by byte.
    utf16: Vec<u32>,
}

impl Offsets {
    fn of(text: &str) -> Self {
        let mut line_starts = vec![0];
        let mut utf16 = vec![0u32; text.len() + 1];
        let mut count = 0u32;
        for (at, ch) in text.char_indices() {
            utf16[at] = count;
            count += ch.len_utf16() as u32;
            if ch == '\n' {
                line_starts.push(at + 1);
            }
        }
        utf16[text.len()] = count;
        Self { line_starts, utf16 }
    }

    fn utf16(&self, at: usize) -> usize {
        self.utf16[at.min(self.utf16.len() - 1)] as usize
    }

    fn line_column(&self, text: &str, at: usize) -> (usize, usize) {
        let line = self.line_starts.partition_point(|start| *start <= at);
        let start = self.line_starts[line - 1];
        (line, text[start..at].chars().count() + 1)
    }
}

/// The five predefined entities and character references expanded.
#[must_use]
pub fn unescape(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';').filter(|end| *end <= 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let expanded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match expanded {
            Some(ch) => {
                out.push(ch);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::super::{apply, Locator, Op};
    use super::*;

    const TEXT: &str = "<?xml version=\"1.0\"?>\n<!-- head -->\n<entities>\n    <!-- a comment -->\n    <entity entity-name=\"A&amp;B\" package=\"p\">\n        <field name=\"x\"><description><![CDATA[x < y]]></description></field>\n        <?pi here?>\n        <field name=\"\u{1F600}\"/>\n    </entity>\n</entities>\n";

    #[test]
    fn the_outline_has_paths_lines_and_values() {
        let root = outline(TEXT).unwrap();
        assert_eq!(root.name, "entities");
        assert_eq!(root.line, 3);
        let entity = &root.children[0];
        assert_eq!(entity.path, vec![0]);
        assert_eq!(entity.attr("entity-name"), Some("A&B"));
        assert_eq!(entity.line, 5);
        assert_eq!(entity.column, 5);
        let description = &entity.children[0].children[0];
        assert_eq!(description.path, vec![0, 0, 0]);
        assert_eq!(description.text.as_deref(), Some("x < y"));
        assert_eq!(entity.children[1].path, vec![0, 1]);
    }

    /// Each outline path names, for the operations, the element the outline put there: comments, processing
    /// instructions and CDATA are not counted as children.
    #[test]
    fn the_paths_are_the_locators_the_operations_take() {
        let root = outline(TEXT).unwrap();
        let mut stack = vec![&root];
        while let Some(node) = stack.pop() {
            let applied = apply(
                TEXT,
                &[Op::SetAttribute {
                    at: Locator::Path(node.path.clone()),
                    name: "mark".into(),
                    value: "1".into(),
                }],
            )
            .unwrap();
            let marked = outline(&applied.text).unwrap();
            let mut at = &marked;
            for index in &node.path {
                at = &at.children[*index];
            }
            assert_eq!(at.name, node.name);
            assert_eq!(at.attr("mark"), Some("1"), "{:?}", node.path);
            stack.extend(node.children.iter());
        }
    }

    #[test]
    fn spans_count_utf16() {
        let root = outline(TEXT).unwrap();
        let last = &root.children[0].children[1];
        let from = TEXT.find("<field name=\"\u{1F600}").unwrap();
        let expected: usize = TEXT[..from].chars().map(char::len_utf16).sum();
        assert_eq!(last.from16, expected);
        assert_eq!(
            last.to16 - last.from16,
            "<field name=\"\u{1F600}\"/>".encode_utf16().count()
        );
    }

    #[test]
    fn entities_expand() {
        assert_eq!(unescape("a &lt;b&gt; &#65;&#x42; &bogus; &"), "a <b> AB &bogus; &");
    }
}
