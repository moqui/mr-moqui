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

//! Markdown to and from a co-edited document: the `default` XML fragment of a Yjs document as `y-prosemirror` keeps a
//! Tiptap editor's content (`m-coedit`), so the server seeds the document from a page's text and writes the page's text
//! from it, and a browser editor reads and writes the same structure.
//!
//! Each node is an XML element named by its Tiptap type with its attributes (`heading` `level`, `orderedList` `start`,
//! `codeBlock` `language`, `image` `src` `alt` `title`, `tableCell` `colspan` `rowspan`); the text of a block is XML text
//! whose formatting attributes are the marks, each its name to its attributes (`bold: {}`, `link: {href}`). The nodes:
//! `paragraph`, `heading`, `blockquote`, `bulletList`, `orderedList`, `listItem`, `codeBlock`, `horizontalRule`,
//! `hardBreak`, `image`, `table`, `tableRow`, `tableHeader`, `tableCell`; the marks: `bold`, `italic`, `strike`,
//! `underline`, `code`, `link`. HTML in the markdown stays as its text, which is written back as it is, as is a wiki
//! link (`[[Page]]`); the markdown written is normalized (`-` bullets, `**` and `*`, fenced code), so a page that is opened
//! and saved without a change keeps its own text (the save compares).

use std::collections::HashMap;
use std::sync::Arc;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use yrs::types::text::Diff;
use yrs::types::xml::{XmlElementPrelim, XmlFragment, XmlOut, XmlTextPrelim};
use yrs::{Any, Doc, GetString, Out, ReadTxn, Text, Transact, TransactionMut, Xml, XmlElementRef};

/// The fragment the editor binds (Tiptap Collaboration's default `field`).
pub const FRAGMENT: &str = "default";

// ---------- the parsed tree ----------

#[derive(Clone, Debug, Default, PartialEq)]
struct Marks {
    bold: bool,
    italic: bool,
    strike: bool,
    underline: bool,
    code: bool,
    link: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
enum Inline {
    Text(String, Marks),
    HardBreak,
}

#[derive(Clone, Debug, PartialEq)]
enum Block {
    Paragraph(Vec<Inline>),
    Heading(u8, Vec<Inline>),
    Blockquote(Vec<Block>),
    BulletList(Vec<Vec<Block>>),
    OrderedList(u64, Vec<Vec<Block>>),
    CodeBlock(Option<String>, String),
    Rule,
    Image {
        src: String,
        alt: String,
        title: String,
    },
    /// Rows, the first the header when `header` is true; each cell its inlines.
    Table {
        header: bool,
        rows: Vec<Vec<Vec<Inline>>>,
    },
}

/// What is being built while the events come.
enum Open {
    Blocks(Vec<Block>),
    Quote(Vec<Block>),
    List(Option<u64>, Vec<Vec<Block>>),
    Item(Vec<Block>),
    Inlines(InlineKind, Vec<Inline>),
    Code(Option<String>, String),
    Table(Vec<Vec<Vec<Inline>>>, bool),
    Row(Vec<Vec<Inline>>),
}

#[derive(Clone, Copy, PartialEq)]
enum InlineKind {
    Paragraph,
    Heading(u8),
    Cell,
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// The blocks of a markdown text.
fn parse(markdown: &str) -> Vec<Block> {
    let mut stack: Vec<Open> = vec![Open::Blocks(Vec::new())];
    let mut marks = Marks::default();
    // inline content a list item or a cell gets without a paragraph event (a tight list)
    let ensure_inlines = |stack: &mut Vec<Open>| {
        if !matches!(stack.last(), Some(Open::Inlines(..))) {
            stack.push(Open::Inlines(InlineKind::Paragraph, Vec::new()));
        }
    };
    let push_block = |stack: &mut Vec<Open>, block: Block| match stack.last_mut() {
        Some(Open::Blocks(list) | Open::Quote(list) | Open::Item(list)) => list.push(block),
        _ => {
            // a block where inlines were open (an image in a tight item): close them first
            if let Some(Open::Inlines(kind, inlines)) = stack.pop() {
                if let Some(Open::Blocks(list) | Open::Quote(list) | Open::Item(list)) = stack.last_mut() {
                    if !inlines.is_empty() {
                        list.push(inline_block(kind, inlines));
                    }
                    list.push(block);
                }
            }
        }
    };
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    if matches!(stack.last(), Some(Open::Inlines(..))) {
                        close_inlines(&mut stack);
                    }
                    stack.push(Open::Inlines(InlineKind::Paragraph, Vec::new()));
                }
                Tag::Heading { level, .. } => {
                    stack.push(Open::Inlines(InlineKind::Heading(heading_level(level)), Vec::new()))
                }
                Tag::BlockQuote(_) => stack.push(Open::Quote(Vec::new())),
                Tag::List(start) => {
                    if matches!(stack.last(), Some(Open::Inlines(..))) {
                        close_inlines(&mut stack);
                    }
                    stack.push(Open::List(start, Vec::new()));
                }
                Tag::Item => stack.push(Open::Item(Vec::new())),
                Tag::CodeBlock(kind) => {
                    let language = match kind {
                        CodeBlockKind::Fenced(info) => info
                            .split_whitespace()
                            .next()
                            .map(str::to_string)
                            .filter(|text| !text.is_empty()),
                        CodeBlockKind::Indented => None,
                    };
                    stack.push(Open::Code(language, String::new()));
                }
                Tag::Table(_) => stack.push(Open::Table(Vec::new(), false)),
                Tag::TableHead => {
                    if let Some(Open::Table(_, header)) = stack.last_mut() {
                        *header = true;
                    }
                    stack.push(Open::Row(Vec::new()));
                }
                Tag::TableRow => stack.push(Open::Row(Vec::new())),
                Tag::TableCell => stack.push(Open::Inlines(InlineKind::Cell, Vec::new())),
                Tag::Emphasis => marks.italic = true,
                Tag::Strong => marks.bold = true,
                Tag::Strikethrough => marks.strike = true,
                Tag::Link { dest_url, .. } => marks.link = Some(dest_url.to_string()),
                Tag::Image { dest_url, title, .. } => {
                    // the image's alt text comes as the text events up to its end
                    stack.push(Open::Code(
                        Some(format!("\u{0}image\u{0}{dest_url}\u{0}{title}")),
                        String::new(),
                    ));
                }
                Tag::HtmlBlock => stack.push(Open::Inlines(InlineKind::Paragraph, Vec::new())),
                _ => {}
            },
            Event::End(end) => match end {
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::HtmlBlock => close_inlines(&mut stack),
                TagEnd::BlockQuote(_) => {
                    if matches!(stack.last(), Some(Open::Inlines(..))) {
                        close_inlines(&mut stack);
                    }
                    if let Some(Open::Quote(blocks)) = stack.pop() {
                        push_block(&mut stack, Block::Blockquote(blocks));
                    }
                }
                TagEnd::Item => {
                    if matches!(stack.last(), Some(Open::Inlines(..))) {
                        close_inlines(&mut stack);
                    }
                    if let Some(Open::Item(blocks)) = stack.pop() {
                        if let Some(Open::List(_, items)) = stack.last_mut() {
                            items.push(if blocks.is_empty() {
                                vec![Block::Paragraph(Vec::new())]
                            } else {
                                blocks
                            });
                        }
                    }
                }
                TagEnd::List(_) => {
                    if let Some(Open::List(start, items)) = stack.pop() {
                        let block = match start {
                            Some(start) => Block::OrderedList(start, items),
                            None => Block::BulletList(items),
                        };
                        push_block(&mut stack, block);
                    }
                }
                TagEnd::CodeBlock => {
                    if let Some(Open::Code(language, mut text)) = stack.pop() {
                        if text.ends_with('\n') {
                            text.pop();
                        }
                        push_block(&mut stack, Block::CodeBlock(language, text));
                    }
                }
                TagEnd::Image => {
                    if let Some(Open::Code(Some(info), alt)) = stack.pop() {
                        let mut parts = info.trim_start_matches("\u{0}image\u{0}").splitn(2, '\u{0}');
                        let src = parts.next().unwrap_or_default().to_string();
                        let title = parts.next().unwrap_or_default().to_string();
                        // an image is a block of the editor: the text before it in its paragraph is a paragraph of its own
                        let image = Block::Image { src, alt, title };
                        if let Some(Open::Inlines(kind, inlines)) = stack.last_mut() {
                            let before = std::mem::take(inlines);
                            let kind = *kind;
                            if let Some(Open::Inlines(..)) = stack.pop() {}
                            if !before.is_empty() {
                                push_block(&mut stack, inline_block(kind, before));
                            }
                            push_block(&mut stack, image);
                            stack.push(Open::Inlines(kind, Vec::new()));
                        } else {
                            push_block(&mut stack, image);
                        }
                    }
                }
                TagEnd::TableCell => {
                    if let Some(Open::Inlines(_, inlines)) = stack.pop() {
                        if let Some(Open::Row(cells)) = stack.last_mut() {
                            cells.push(inlines);
                        }
                    }
                }
                TagEnd::TableHead | TagEnd::TableRow => {
                    if let Some(Open::Row(cells)) = stack.pop() {
                        if let Some(Open::Table(rows, _)) = stack.last_mut() {
                            rows.push(cells);
                        }
                    }
                }
                TagEnd::Table => {
                    if let Some(Open::Table(rows, header)) = stack.pop() {
                        push_block(&mut stack, Block::Table { header, rows });
                    }
                }
                TagEnd::Emphasis => marks.italic = false,
                TagEnd::Strong => marks.bold = false,
                TagEnd::Strikethrough => marks.strike = false,
                TagEnd::Link => marks.link = None,
                _ => {}
            },
            Event::Text(text) => match stack.last_mut() {
                Some(Open::Code(_, code)) => code.push_str(&text),
                _ => {
                    ensure_inlines(&mut stack);
                    push_text(&mut stack, &text, &marks);
                }
            },
            Event::Code(text) => {
                ensure_inlines(&mut stack);
                let mut code = marks.clone();
                code.code = true;
                push_text(&mut stack, &text, &code);
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                // HTML stays as its text; a block of it ends with the newline the markdown had
                let html = html.to_string();
                if let Some(Open::Code(_, code)) = stack.last_mut() {
                    code.push_str(&html);
                    continue;
                }
                ensure_inlines(&mut stack);
                let mut lines = html.trim_end_matches('\n').split('\n').peekable();
                while let Some(line) = lines.next() {
                    push_text(&mut stack, line, &Marks::default());
                    if lines.peek().is_some() {
                        if let Some(Open::Inlines(_, inlines)) = stack.last_mut() {
                            inlines.push(Inline::HardBreak);
                        }
                    }
                }
            }
            Event::SoftBreak => {
                ensure_inlines(&mut stack);
                push_text(&mut stack, " ", &marks);
            }
            Event::HardBreak => {
                ensure_inlines(&mut stack);
                if let Some(Open::Inlines(_, inlines)) = stack.last_mut() {
                    inlines.push(Inline::HardBreak);
                }
            }
            Event::Rule => push_block(&mut stack, Block::Rule),
            Event::TaskListMarker(done) => {
                ensure_inlines(&mut stack);
                push_text(&mut stack, if done { "[x] " } else { "[ ] " }, &Marks::default());
            }
            Event::FootnoteReference(name) => {
                ensure_inlines(&mut stack);
                push_text(&mut stack, &format!("[^{name}]"), &Marks::default());
            }
            _ => {}
        }
    }
    while stack.len() > 1 {
        match stack.last() {
            Some(Open::Inlines(..)) => close_inlines(&mut stack),
            _ => {
                stack.pop();
            }
        }
    }
    match stack.pop() {
        Some(Open::Blocks(blocks)) => blocks,
        _ => Vec::new(),
    }
}

fn inline_block(kind: InlineKind, inlines: Vec<Inline>) -> Block {
    match kind {
        InlineKind::Heading(level) => Block::Heading(level, inlines),
        _ => Block::Paragraph(inlines),
    }
}

fn push_text(stack: &mut [Open], text: &str, marks: &Marks) {
    let Some(Open::Inlines(_, inlines)) = stack.last_mut() else {
        return;
    };
    if let Some(Inline::Text(last, last_marks)) = inlines.last_mut() {
        if last_marks == marks {
            last.push_str(text);
            return;
        }
    }
    if !text.is_empty() {
        inlines.push(Inline::Text(text.to_string(), marks.clone()));
    }
}

fn close_inlines(stack: &mut Vec<Open>) {
    let Some(Open::Inlines(kind, inlines)) = stack.pop() else {
        return;
    };
    if kind == InlineKind::Cell {
        stack.push(Open::Inlines(kind, inlines));
        return;
    }
    // a paragraph that held only an image (now taken out) leaves nothing
    if inlines.is_empty()
        && matches!(kind, InlineKind::Paragraph)
        && matches!(
            stack.last(),
            Some(Open::Item(_)) | Some(Open::Blocks(_)) | Some(Open::Quote(_))
        )
    {
        return;
    }
    if let Some(Open::Blocks(list) | Open::Quote(list) | Open::Item(list)) = stack.last_mut() {
        list.push(inline_block(kind, inlines));
    }
}

// ---------- into the document ----------

/// The formatting attributes of a run: each mark it has, and null for each it has not, as text inserted after a run
/// otherwise takes that run's marks.
fn mark_attributes(marks: &Marks) -> HashMap<Arc<str>, Any> {
    let empty = || Any::Map(Arc::new(HashMap::new()));
    let mut out = HashMap::new();
    for (on, name) in [
        (marks.bold, "bold"),
        (marks.italic, "italic"),
        (marks.strike, "strike"),
        (marks.underline, "underline"),
        (marks.code, "code"),
    ] {
        out.insert(Arc::from(name), if on { empty() } else { Any::Null });
    }
    let link = marks.link.as_ref().map_or(Any::Null, |href| {
        let mut link = HashMap::new();
        link.insert("href".to_string(), Any::String(Arc::from(href.as_str())));
        Any::Map(Arc::new(link))
    });
    out.insert(Arc::from("link"), link);
    out
}

/// Write `inlines` into an element: text runs into XML texts with their marks, a hard break as its element between them.
fn write_inlines<F: XmlFragment>(txn: &mut TransactionMut, parent: &F, inlines: &[Inline]) {
    let mut text: Option<yrs::XmlTextRef> = None;
    for inline in inlines {
        match inline {
            Inline::Text(chunk, marks) => {
                let target = text.get_or_insert_with(|| parent.push_back(txn, XmlTextPrelim::new("")));
                let at = target.len(txn);
                target.insert_with_attributes(txn, at, chunk, mark_attributes(marks));
            }
            Inline::HardBreak => {
                text = None;
                parent.push_back(txn, XmlElementPrelim::empty("hardBreak"));
            }
        }
    }
}

fn element(txn: &mut TransactionMut, parent: &impl XmlFragment, name: &str) -> XmlElementRef {
    parent.push_back(txn, XmlElementPrelim::empty(name))
}

fn write_blocks(txn: &mut TransactionMut, parent: &impl XmlFragment, blocks: &[Block]) {
    for block in blocks {
        match block {
            Block::Paragraph(inlines) => {
                let node = element(txn, parent, "paragraph");
                write_inlines(txn, &node, inlines);
            }
            Block::Heading(level, inlines) => {
                let node = element(txn, parent, "heading");
                node.insert_attribute(txn, "level", Any::Number(yrs::Number::Float(f64::from(*level))));
                write_inlines(txn, &node, inlines);
            }
            Block::Blockquote(blocks) => {
                let node = element(txn, parent, "blockquote");
                write_blocks(txn, &node, blocks);
            }
            Block::BulletList(items) | Block::OrderedList(_, items) => {
                let node = element(
                    txn,
                    parent,
                    if matches!(block, Block::BulletList(_)) {
                        "bulletList"
                    } else {
                        "orderedList"
                    },
                );
                if let Block::OrderedList(start, _) = block {
                    node.insert_attribute(txn, "start", Any::Number(yrs::Number::Float(*start as f64)));
                }
                for item in items {
                    let item_node = element(txn, &node, "listItem");
                    // a list item starts with a paragraph
                    if !matches!(item.first(), Some(Block::Paragraph(_))) {
                        element(txn, &item_node, "paragraph");
                    }
                    write_blocks(txn, &item_node, item);
                }
            }
            Block::CodeBlock(language, text) => {
                let node = element(txn, parent, "codeBlock");
                if let Some(language) = language {
                    node.insert_attribute(txn, "language", Any::String(Arc::from(language.as_str())));
                }
                if !text.is_empty() {
                    node.push_back(txn, XmlTextPrelim::new(text.as_str()));
                }
            }
            Block::Rule => {
                element(txn, parent, "horizontalRule");
            }
            Block::Image { src, alt, title } => {
                let node = element(txn, parent, "image");
                node.insert_attribute(txn, "src", Any::String(Arc::from(src.as_str())));
                if !alt.is_empty() {
                    node.insert_attribute(txn, "alt", Any::String(Arc::from(alt.as_str())));
                }
                if !title.is_empty() {
                    node.insert_attribute(txn, "title", Any::String(Arc::from(title.as_str())));
                }
            }
            Block::Table { header, rows } => {
                let node = element(txn, parent, "table");
                for (index, row) in rows.iter().enumerate() {
                    let row_node = element(txn, &node, "tableRow");
                    for cell in row {
                        let cell_node = element(
                            txn,
                            &row_node,
                            if *header && index == 0 {
                                "tableHeader"
                            } else {
                                "tableCell"
                            },
                        );
                        cell_node.insert_attribute(txn, "colspan", Any::Number(yrs::Number::Float(1.0)));
                        cell_node.insert_attribute(txn, "rowspan", Any::Number(yrs::Number::Float(1.0)));
                        let paragraph = element(txn, &cell_node, "paragraph");
                        write_inlines(txn, &paragraph, cell);
                    }
                }
            }
        }
    }
}

/// Fill the empty document's fragment with a markdown text.
pub fn seed(doc: &Doc, markdown: &str) {
    let fragment = doc.get_or_insert_xml_fragment(FRAGMENT);
    let blocks = parse(markdown);
    let mut txn = doc.transact_mut();
    if fragment.len(&txn) > 0 {
        return;
    }
    if blocks.is_empty() {
        element(&mut txn, &fragment, "paragraph");
        return;
    }
    write_blocks(&mut txn, &fragment, &blocks);
}

// ---------- out of the document ----------

/// The markdown of the document's fragment.
pub fn render(doc: &Doc) -> String {
    let fragment = doc.get_or_insert_xml_fragment(FRAGMENT);
    let txn = doc.transact();
    let mut out = String::new();
    render_blocks(&txn, &fragment, "", &mut out);
    let trimmed = out.trim_end_matches('\n');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    }
}

fn number_attribute<T: ReadTxn>(txn: &T, node: &XmlElementRef, name: &str) -> Option<f64> {
    match node.get_attribute(txn, name)? {
        Out::Any(Any::Number(yrs::Number::Float(value))) => Some(value),
        Out::Any(Any::Number(yrs::Number::Int(value))) => Some(value as f64),
        Out::Any(Any::String(text)) => text.parse().ok(),
        _ => None,
    }
}

fn text_attribute<T: ReadTxn>(txn: &T, node: &XmlElementRef, name: &str) -> Option<String> {
    match node.get_attribute(txn, name)? {
        Out::Any(Any::String(text)) => Some(text.to_string()),
        Out::Any(Any::Null | Any::Undefined) => None,
        Out::Any(other) => Some(other.to_string()),
        _ => None,
    }
}

fn children<T: ReadTxn>(txn: &T, node: &impl XmlFragment) -> Vec<XmlOut> {
    node.children(txn).collect()
}

/// `prefix` begins each line after the first of a block (a quote's `> `, a list item's indent).
fn render_blocks<T: ReadTxn>(txn: &T, parent: &impl XmlFragment, prefix: &str, out: &mut String) {
    render_blocks_in(txn, parent, prefix, out, false);
}

fn render_blocks_in<T: ReadTxn>(txn: &T, parent: &impl XmlFragment, prefix: &str, out: &mut String, in_item: bool) {
    let mut first = true;
    let mut previous: Option<String> = None;
    let mut alternated = false;
    for child in children(txn, parent) {
        let XmlOut::Element(node) = child else {
            // text outside a block: a paragraph of it
            if let XmlOut::Text(text) = child {
                let inline = render_text(txn, &text);
                if !inline.trim().is_empty() {
                    push_block_text(out, prefix, &mut first, &inline);
                }
            }
            continue;
        };
        let tag = node.tag().to_string();
        // a list right after a list of its kind takes the other marker, or markdown would read the two as one
        let alternate = previous.as_deref() == Some(tag.as_str())
            && matches!(tag.as_str(), "bulletList" | "orderedList")
            && !alternated;
        alternated = alternate;
        let block = render_block(txn, &node, &tag, alternate);
        if let Some(block) = block {
            // in a list item a nested list follows its paragraph on the next line, so the list stays tight
            let tight = in_item
                && previous.as_deref() == Some("paragraph")
                && matches!(tag.as_str(), "bulletList" | "orderedList" | "taskList");
            if tight {
                out.push_str(prefix);
                let mut lines = block.split('\n');
                if let Some(line) = lines.next() {
                    out.push_str(line);
                }
                for line in lines {
                    out.push('\n');
                    out.push_str(prefix);
                    out.push_str(line);
                }
                out.push('\n');
                first = false;
            } else {
                push_block_text(out, prefix, &mut first, &block);
            }
            previous = Some(tag);
        }
    }
}

/// Add a block's text: blocks are parted by a blank line; every line but the very first gets the prefix.
fn push_block_text(out: &mut String, prefix: &str, first: &mut bool, block: &str) {
    if !*first {
        out.push_str(prefix.trim_end());
        out.push('\n');
        out.push_str(prefix);
    }
    *first = false;
    let mut lines = block.split('\n');
    if let Some(line) = lines.next() {
        out.push_str(line);
    }
    for line in lines {
        out.push('\n');
        if line.is_empty() {
            out.push_str(prefix.trim_end());
        } else {
            out.push_str(prefix);
            out.push_str(line);
        }
    }
    out.push('\n');
}

fn render_block<T: ReadTxn>(txn: &T, node: &XmlElementRef, tag: &str, alternate: bool) -> Option<String> {
    Some(match tag {
        "paragraph" => render_inlines(txn, node, true),
        "heading" => {
            let level = number_attribute(txn, node, "level").unwrap_or(1.0).clamp(1.0, 6.0) as usize;
            format!("{} {}", "#".repeat(level), render_inlines(txn, node, false))
        }
        "blockquote" => {
            let mut inner = String::new();
            render_blocks(txn, node, "> ", &mut inner);
            format!("> {}", inner.trim_end_matches('\n'))
        }
        "bulletList" | "orderedList" | "taskList" => {
            let ordered = tag == "orderedList";
            let mut number = number_attribute(txn, node, "start").unwrap_or(1.0) as u64;
            let mut items = Vec::new();
            for item in children(txn, node) {
                let XmlOut::Element(item) = item else { continue };
                let marker = match (ordered, alternate) {
                    (true, false) => format!("{number}. "),
                    (true, true) => format!("{number}) "),
                    (false, false) => "- ".to_string(),
                    (false, true) => "* ".to_string(),
                };
                number += 1;
                let indent = " ".repeat(marker.len());
                let mut inner = String::new();
                render_blocks_in(txn, &item, &indent, &mut inner, true);
                items.push(format!("{marker}{}", inner.trim_end_matches('\n')));
            }
            items.join("\n")
        }
        "codeBlock" => {
            let language = text_attribute(txn, node, "language").unwrap_or_default();
            let code: String = children(txn, node)
                .into_iter()
                .filter_map(|child| match child {
                    XmlOut::Text(text) => Some(text.get_string(txn)),
                    _ => None,
                })
                .collect();
            let fence = if code.contains("```") { "~~~~" } else { "```" };
            format!("{fence}{language}\n{code}\n{fence}")
        }
        "horizontalRule" => "---".to_string(),
        "image" => {
            let src = text_attribute(txn, node, "src").unwrap_or_default();
            let alt = text_attribute(txn, node, "alt").unwrap_or_default();
            match text_attribute(txn, node, "title").filter(|title| !title.is_empty()) {
                Some(title) => format!("![{}]({src} \"{}\")", escape_text(&alt), title.replace('"', "\\\"")),
                None => format!("![{}]({src})", escape_text(&alt)),
            }
        }
        "table" => render_table(txn, node),
        // a node this mapping does not know: its text, so nothing written is lost
        _ => {
            let mut inner = String::new();
            render_blocks(txn, node, "", &mut inner);
            let inner = inner.trim_end_matches('\n').to_string();
            if inner.is_empty() {
                render_inlines(txn, node, true)
            } else {
                inner
            }
        }
    })
}

fn render_table<T: ReadTxn>(txn: &T, node: &XmlElementRef) -> String {
    let mut rows: Vec<(bool, Vec<String>)> = Vec::new();
    for row in children(txn, node) {
        let XmlOut::Element(row) = row else { continue };
        let mut cells = Vec::new();
        let mut header = true;
        for cell in children(txn, &row) {
            let XmlOut::Element(cell) = cell else { continue };
            header &= cell.tag().as_ref() == "tableHeader";
            let mut text = String::new();
            for part in children(txn, &cell) {
                if let XmlOut::Element(part) = part {
                    if !text.is_empty() {
                        text.push_str("<br>");
                    }
                    text.push_str(&render_inlines(txn, &part, false).replace('\n', " "));
                }
            }
            cells.push(text.replace('|', "\\|"));
        }
        rows.push((header && !cells.is_empty(), cells));
    }
    let columns = rows.iter().map(|(_, cells)| cells.len()).max().unwrap_or(1).max(1);
    let line = |cells: &[String]| {
        let mut padded: Vec<String> = cells.to_vec();
        padded.resize(columns, String::new());
        format!("| {} |", padded.join(" | "))
    };
    let mut out = Vec::new();
    let mut body = rows.as_slice();
    match rows.first() {
        Some((true, cells)) => {
            out.push(line(cells));
            body = &rows[1..];
        }
        _ => out.push(line(&vec![String::new(); columns])),
    }
    out.push(format!("|{}", " --- |".repeat(columns)));
    for (_, cells) in body {
        out.push(line(cells));
    }
    out.join("\n")
}

/// The marks of a run of text, from its formatting attributes.
fn marks_of(attributes: Option<&HashMap<Arc<str>, Any>>) -> Marks {
    let mut marks = Marks::default();
    let Some(attributes) = attributes else { return marks };
    for (name, value) in attributes {
        // y-prosemirror names a second mark of one type `name--hash`
        let base = name.split("--").next().unwrap_or(name);
        let on = !matches!(value, Any::Null | Any::Undefined | Any::Bool(false));
        match base {
            "bold" => marks.bold |= on,
            "italic" => marks.italic |= on,
            "strike" => marks.strike |= on,
            "underline" => marks.underline |= on,
            "code" => marks.code |= on,
            "link" => {
                if let Any::Map(map) = value {
                    if let Some(Any::String(href)) = map.get("href") {
                        marks.link = Some(href.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    marks
}

fn render_text<T: ReadTxn>(txn: &T, text: &yrs::XmlTextRef) -> String {
    let runs: Vec<(String, Marks)> = text
        .diff(txn, |_| ())
        .into_iter()
        .filter_map(|Diff { insert, attributes, .. }| match insert {
            Out::Any(Any::String(chunk)) => Some((chunk.to_string(), marks_of(attributes.as_deref()))),
            _ => None,
        })
        .collect();
    render_runs(&runs)
}

/// The inline markdown of a block's children: texts with their marks, hard breaks as a backslash and a new line.
fn render_inlines<T: ReadTxn>(txn: &T, node: &XmlElementRef, _paragraph: bool) -> String {
    let mut out = String::new();
    for child in children(txn, node) {
        match child {
            XmlOut::Text(text) => out.push_str(&render_text(txn, &text)),
            XmlOut::Element(element) if element.tag().as_ref() == "hardBreak" => out.push_str("\\\n"),
            XmlOut::Element(element) => out.push_str(&render_inlines(txn, &element, false)),
            XmlOut::Fragment(_) => {}
        }
    }
    out
}

/// Runs of text as markdown: the marks opened and closed in a stable order (link outside, then bold, italic, strike,
/// underline), so a run that keeps a mark of the one before it does not close and open it again.
fn render_runs(runs: &[(String, Marks)]) -> String {
    let mut out = String::new();
    let mut index = 0;
    while index < runs.len() {
        // a link spans the runs that share its address
        let link = runs[index].1.link.clone();
        let end = runs[index..]
            .iter()
            .position(|(_, marks)| marks.link != link)
            .map_or(runs.len(), |offset| index + offset);
        let inner = render_unlinked(&runs[index..end]);
        match link {
            Some(href) => out.push_str(&format!("[{inner}]({})", link_destination(&href))),
            None => out.push_str(&inner),
        }
        index = end;
    }
    out
}

fn link_destination(href: &str) -> String {
    if href.contains(' ') || href.contains('(') || href.contains(')') {
        format!("<{}>", href.replace('>', "%3E"))
    } else {
        href.to_string()
    }
}

fn render_unlinked(runs: &[(String, Marks)]) -> String {
    const ORDER: [(&str, &str); 4] = [("bold", "**"), ("italic", "*"), ("strike", "~~"), ("underline", "<u>")];
    let has = |marks: &Marks, name: &str| match name {
        "bold" => marks.bold,
        "italic" => marks.italic,
        "strike" => marks.strike,
        _ => marks.underline,
    };
    let close_of = |name: &str| {
        if name == "underline" {
            "</u>".to_string()
        } else {
            ORDER
                .iter()
                .find(|(known, _)| *known == name)
                .map_or(String::new(), |(_, token)| token.to_string())
        }
    };
    let mut out = String::new();
    let mut open: Vec<&str> = Vec::new();
    for (text, marks) in runs {
        // close what this run does not have, and what was opened after it
        if let Some(depth) = open.iter().position(|name| !has(marks, name)) {
            while open.len() > depth {
                let name = open.pop().unwrap_or_default();
                out.push_str(&close_of(name));
            }
        }
        for (name, token) in ORDER {
            if has(marks, name) && !open.contains(&name) {
                out.push_str(token);
                open.push(name);
            }
        }
        if marks.code {
            let fence = if text.contains('`') { "`` " } else { "`" };
            let end = if text.contains('`') { " ``" } else { "`" };
            out.push_str(&format!("{fence}{text}{end}"));
        } else {
            out.push_str(&escape_text(text));
        }
    }
    while let Some(name) = open.pop() {
        out.push_str(&close_of(name));
    }
    out
}

/// Text escaped only where markdown would read it as formatting: `\`, `*`, `` ` ``, `~~`, `_` at a word's edge, a
/// bracket pair followed by `(` (a link), and what would start a block at the start of a line. HTML and wiki links stay.
fn escape_text(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (index, ch) in chars.iter().enumerate() {
        let before = index.checked_sub(1).map(|at| chars[at]);
        let after = chars.get(index + 1).copied();
        let at_line_start = index == 0 || before == Some('\n');
        match ch {
            '\\' | '*' | '`' => out.push('\\'),
            '~' if after == Some('~') || before == Some('~') => out.push('\\'),
            '_' if !(before.is_some_and(char::is_alphanumeric) && after.is_some_and(char::is_alphanumeric)) => {
                out.push('\\')
            }
            '[' if link_ahead(&chars[index..]) => out.push('\\'),
            '#' | '>' | '+' | '=' if at_line_start => out.push('\\'),
            '-' if at_line_start && (after.is_none_or(char::is_whitespace) || after == Some('-')) => out.push('\\'),
            _ => {}
        }
        // `1. ` at the start of a line would start a numbered list
        if matches!(ch, '.' | ')') && after.is_none_or(char::is_whitespace) {
            let digits = chars[..index].iter().rev().take_while(|c| c.is_ascii_digit()).count();
            if digits > 0 && (digits == index || chars[index - digits - 1] == '\n') {
                out.push('\\');
            }
        }
        out.push(*ch);
    }
    out
}

/// Whether `[text](` starts here.
fn link_ahead(chars: &[char]) -> bool {
    let Some(close) = chars.iter().position(|ch| *ch == ']') else {
        return false;
    };
    chars.get(close + 1) == Some(&'(')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(markdown: &str) -> String {
        let doc = Doc::new();
        seed(&doc, markdown);
        render(&doc)
    }

    /// The structure y-prosemirror reads: element names, attributes and the marks of each text run.
    fn outline(doc: &Doc) -> String {
        fn walk<T: ReadTxn>(txn: &T, node: &impl XmlFragment, out: &mut String) {
            for child in node.children(txn) {
                match child {
                    XmlOut::Element(element) => {
                        out.push('<');
                        out.push_str(element.tag());
                        let mut attributes: Vec<(String, String)> = element
                            .attributes(txn)
                            .map(|(key, value)| (key.to_string(), value.to_string(txn)))
                            .collect();
                        attributes.sort();
                        for (key, value) in attributes {
                            out.push_str(&format!(" {key}={value}"));
                        }
                        out.push('>');
                        walk(txn, &element, out);
                        out.push_str(&format!("</{}>", element.tag()));
                    }
                    XmlOut::Text(text) => {
                        for diff in text.diff(txn, |_| ()) {
                            let mut names: Vec<String> = diff
                                .attributes
                                .map(|map| map.keys().map(|key| key.to_string()).collect())
                                .unwrap_or_default();
                            names.sort();
                            out.push_str(&format!("[{}]{}", names.join(","), diff.insert.to_string(txn)));
                        }
                    }
                    XmlOut::Fragment(_) => {}
                }
            }
        }
        let fragment = doc.get_or_insert_xml_fragment(FRAGMENT);
        let txn = doc.transact();
        let mut out = String::new();
        walk(&txn, &fragment, &mut out);
        out
    }

    #[test]
    fn the_editor_structure_of_a_page() {
        let doc = Doc::new();
        seed(
            &doc,
            "# Title\n\nSome **bold** and [a link](https://example.com).\n\n- one\n- two\n",
        );
        assert_eq!(
            outline(&doc),
            "<heading level=1>[]Title</heading><paragraph>[]Some [bold]bold[] and [link]a link[].</paragraph>\
             <bulletList><listItem><paragraph>[]one</paragraph></listItem><listItem><paragraph>[]two</paragraph></listItem></bulletList>"
        );
    }

    #[test]
    fn normalized_markdown_reads_back_as_it_was() {
        for text in [
            "# Title\n\nA paragraph with **bold**, *italic*, ~~gone~~ and `code`.\n",
            "## Lists\n\n- one\n- two\n  - nested\n\n1. first\n2. second\n",
            "> A quote\n>\n> - in a list\n",
            "```rust\nfn main() {}\n```\n",
            "Line one\\\nline two\n",
            "---\n\n![A cat](/img/cat.png)\n",
            "| A | B |\n| --- | --- |\n| 1 | 2 |\n",
            "A [[Wiki Link]] and [text](Other/Page) and <u>under</u> and snake_case_name.\n",
            "**bold *both* bold**\n",
        ] {
            assert_eq!(round(text), text, "{text}");
        }
    }

    #[test]
    fn markdown_is_normalized_once_then_stable() {
        let source = "Title\n=====\n\n* star bullet\n+ plus bullet\n\n__strong__ and _em_\n\n    indented code\n";
        let once = round(source);
        assert_eq!(
            once,
            "# Title\n\n- star bullet\n\n* plus bullet\n\n**strong** and *em*\n\n```\nindented code\n```\n"
        );
        assert_eq!(round(&once), once);
    }

    #[test]
    fn text_that_looks_like_markdown_is_escaped() {
        let doc = Doc::new();
        let fragment = doc.get_or_insert_xml_fragment(FRAGMENT);
        {
            let mut txn = doc.transact_mut();
            let paragraph = fragment.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
            paragraph.push_back(
                &mut txn,
                XmlTextPrelim::new("# not a heading, 2*3*4, [x](y) and 1. not a list"),
            );
        }
        let text = render(&doc);
        assert_eq!(text, "\\# not a heading, 2\\*3\\*4, \\[x](y) and 1. not a list\n");
        assert_eq!(round(&text), text);
    }

    #[test]
    fn an_empty_page_is_one_empty_paragraph() {
        let doc = Doc::new();
        seed(&doc, "");
        assert_eq!(outline(&doc), "<paragraph></paragraph>");
        assert_eq!(render(&doc), "");
    }
}
