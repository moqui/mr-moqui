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

//! The comments of an XML text by the element they belong to, as they sit in the file: the comments before an element
//! (after its previous sibling), a comment that starts on the line an element ends on, and the comments after an
//! element's last child. The System › Conf screens show the default conf's comments beside its settings this way
//! (docs/tools/Component-Deploy.md, "Viewing and editing the conf").

use serde::Serialize;

use super::tokenize::{Doc, Kind};
use crate::error::Error;

/// The comments of one element.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ElementComments {
    /// The 1-based line of the element's `<`.
    pub line: usize,
    /// The comments between the previous sibling element (or the parent's start tag) and the element.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<String>,
    /// A comment that starts on the line the element ends on.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    /// The comments after the element's last child element.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub trailing: Vec<String>,
}

impl ElementComments {
    fn is_empty(&self) -> bool {
        self.before.is_empty() && self.after.is_empty() && self.trailing.is_empty()
    }
}

/// The comments of `text` by element, in document order, for the elements that have any. The comments before the root
/// element are the root's.
///
/// # Errors
///
/// When `text` does not scan ([`Doc::parse`]).
pub fn element_comments(text: &str) -> Result<Vec<ElementComments>, Error> {
    let doc = Doc::parse(text)?;
    let lines = LineIndex::new(text);
    let mut out: Vec<ElementComments> = vec![ElementComments::default(); doc.nodes.len()];
    for (index, node) in doc.nodes.iter().enumerate() {
        if !matches!(node.kind, Kind::Element | Kind::Document) {
            continue;
        }
        let mut pending: Vec<String> = Vec::new();
        let mut previous: Option<usize> = None;
        for &child in &node.children {
            let child_node = &doc.nodes[child];
            match child_node.kind {
                Kind::Comment => {
                    let comment = comment_text(doc.slice(&child_node.span));
                    let ends_on_same_line = previous.is_some_and(|prev| {
                        let end = doc.nodes[prev].span.end.saturating_sub(1);
                        lines.line(end) == lines.line(child_node.span.start)
                    });
                    match previous {
                        Some(prev) if ends_on_same_line && pending.is_empty() => out[prev].after.push(comment),
                        _ => pending.push(comment),
                    }
                }
                Kind::Element => {
                    out[child].before.append(&mut pending);
                    previous = Some(child);
                }
                _ => {}
            }
        }
        if node.kind == Kind::Element {
            out[index].trailing.append(&mut pending);
        }
    }
    let mut found = Vec::new();
    for (index, mut comments) in out.into_iter().enumerate() {
        if doc.nodes[index].kind != Kind::Element || comments.is_empty() {
            continue;
        }
        comments.line = lines.line(doc.nodes[index].span.start);
        found.push(comments);
    }
    Ok(found)
}

/// The text of a comment, `<!--` and `-->` taken off, its lines without their common indentation, trimmed.
fn comment_text(comment: &str) -> String {
    let inner = comment
        .strip_prefix("<!--")
        .and_then(|rest| rest.strip_suffix("-->"))
        .unwrap_or(comment);
    let lines: Vec<&str> = inner.lines().collect();
    // the first line follows `<!--`; the others are indented in the file
    let indent = lines
        .iter()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut text = String::new();
    for (at, line) in lines.iter().enumerate() {
        let line = if at == 0 {
            line.trim()
        } else {
            line.get(indent..).unwrap_or_else(|| line.trim_start()).trim_end()
        };
        if at > 0 {
            text.push('\n');
        }
        text.push_str(line);
    }
    text.trim().to_string()
}

/// The byte offsets where the lines of a text start, to find a line by offset.
struct LineIndex(Vec<usize>);

impl LineIndex {
    fn new(text: &str) -> Self {
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(at, _)| at + 1))
            .collect();
        Self(starts)
    }

    /// The 1-based line of byte `at`.
    fn line(&self, at: usize) -> usize {
        self.0.partition_point(|start| *start <= at)
    }
}

#[cfg(test)]
#[path = "comments_tests.rs"]
mod tests;
