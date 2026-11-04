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

//! The descriptions of the docs, read again from the source (plans/archive/Autodoc.md, "Findings"): the loaded definitions keep
//! no `<description>` (the index keeps its first sentence), so each file is parsed once a run and each element's
//! `<description>` is kept by the line its element starts on, the line the index has. A screen or a service file has no
//! description element: its comment before the root element is its description, the license comment left out.

use std::collections::HashMap;

use moqui_lib::node::MNode;
use moqui_lib::xml_edit::{Doc, Kind};

/// What one file says of its elements.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDocs {
    /// Each element's `<description>` by the line its element starts on.
    pub by_line: HashMap<u32, String>,
    /// The file's comment before its root element (or first in it), license comments left out.
    pub lead: Option<String>,
}

/// The files read in a run, each once.
#[derive(Debug, Default)]
pub struct Descriptions {
    files: HashMap<String, Option<FileDocs>>,
}

impl Descriptions {
    /// The `<description>` of the element at `line` of the file `path`.
    pub fn at(&mut self, path: &str, line: u32) -> Option<String> {
        self.file(path).and_then(|docs| docs.by_line.get(&line).cloned())
    }

    /// The comment that heads the file `path`.
    pub fn lead(&mut self, path: &str) -> Option<String> {
        self.file(path).and_then(|docs| docs.lead.clone())
    }

    fn file(&mut self, path: &str) -> Option<&FileDocs> {
        self.files
            .entry(path.to_string())
            .or_insert_with(|| {
                let text = std::fs::read_to_string(path).ok()?;
                match read_text(path, &text) {
                    Ok(docs) => Some(docs),
                    Err(err) => {
                        tracing::warn!("Autodoc: the descriptions of {path} were not read: {err}");
                        None
                    }
                }
            })
            .as_ref()
    }
}

/// The descriptions and the lead comment of the XML `text` (at `location`, for errors).
///
/// # Errors
///
/// Returns the parse error when the text is not well-formed XML.
pub fn read_text(location: &str, text: &str) -> Result<FileDocs, moqui_lib::error::Error> {
    let root = MNode::parse_text(location, text)?;
    let mut docs = FileDocs::default();
    collect(&root, &mut docs.by_line);
    docs.lead = lead_comment(text);
    Ok(docs)
}

fn collect(node: &MNode, by_line: &mut HashMap<u32, String>) {
    for child in node.child_iter() {
        if child.has_name("description") {
            if let Some(text) = child
                .text()
                .map(|text| paragraphs(&text))
                .filter(|text| !text.is_empty())
            {
                by_line.entry(node.line()).or_insert(text);
            }
        } else {
            collect(&child, by_line);
        }
    }
}

/// Indented source text as Markdown paragraphs: each line trimmed, the lines of a paragraph joined by a space, blank lines
/// between paragraphs kept.
#[must_use]
pub fn paragraphs(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            if !current.is_empty() {
                out.push(current.join(" "));
                current.clear();
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        out.push(current.join(" "));
    }
    out.join("\n\n")
}

/// The comments before the root element, else the first comment in it before its first element, license comments left
/// out. A license comment left open holds the next comment's start (`<!-- License ... <!-- About -->`): the part after
/// the last `<!--` is a comment of its own.
#[must_use]
pub fn lead_comment(text: &str) -> Option<String> {
    let doc = Doc::parse(text).ok()?;
    let comment_text = |index: usize| -> &str {
        let raw = doc.slice(&doc.nodes[index].span);
        raw.strip_prefix("<!--")
            .and_then(|inner| inner.strip_suffix("-->"))
            .unwrap_or(raw)
    };
    let mut parts: Vec<String> = Vec::new();
    let take = |inner: &str, parts: &mut Vec<String>| {
        let mut pieces: Vec<&str> = vec![inner];
        if let Some((head, tail)) = inner.rsplit_once("<!--") {
            pieces = vec![head, tail];
        }
        for piece in pieces {
            if is_license(piece) {
                continue;
            }
            let text = paragraphs(piece);
            if !text.is_empty() {
                parts.push(text);
            }
        }
    };
    let root = doc.root();
    for child in &doc.nodes[0].children {
        if Some(*child) == root {
            break;
        }
        if doc.nodes[*child].kind == Kind::Comment {
            take(comment_text(*child), &mut parts);
        }
    }
    if parts.is_empty() {
        if let Some(root) = root {
            for child in &doc.nodes[root].children {
                match doc.nodes[*child].kind {
                    Kind::Comment => {
                        take(comment_text(*child), &mut parts);
                        break;
                    }
                    Kind::Element => break,
                    _ => {}
                }
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// Whether a comment is a license header.
fn is_license(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("public domain")
        || lower.contains("licensed under")
        || lower.contains("license.md")
        || lower.contains("copyright")
        || lower.contains("spdx-license-identifier")
}

#[cfg(test)]
#[path = "describe_tests.rs"]
mod tests;
