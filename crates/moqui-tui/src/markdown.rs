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

//! Markdown as the blocks of an `html` node (`markdown-view`, and the F4 preview of an `editor-type="md"` text area):
//! headings, paragraphs, list items with their markers, quotes, code and rules, with each link's text followed by `[n]`
//! and its url the n-th entry of the links, as the server's HTML conversion writes them. Emphasis is dropped; the text
//! stays.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::model::{clean_text, HtmlBlock, HtmlLink};

/// The blocks and links of `source`.
pub fn blocks(source: &str) -> (Vec<HtmlBlock>, Vec<HtmlLink>) {
    let mut out = Out::default();
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(source, options) {
        out.event(event);
    }
    out.close();
    (out.blocks, out.links)
}

#[derive(Default)]
struct Out {
    blocks: Vec<HtmlBlock>,
    links: Vec<HtmlLink>,
    /// The block being filled: its kind and text, open when `kind` is not empty.
    kind: String,
    text: String,
    marker: String,
    depth: u8,
    quotes: u8,
    /// Each open list: the next number of an ordered one, None for bullets.
    lists: Vec<Option<u64>>,
    /// The link being read: its url, and where its text starts.
    link: Option<(String, usize)>,
    in_code: bool,
    cells: Vec<String>,
}

impl Out {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.push(&text),
            Event::Code(text) => self.push(&text),
            Event::SoftBreak => self.push(" "),
            Event::HardBreak => self.push("\n"),
            Event::Rule => {
                self.close();
                self.blocks.push(block("rule", String::new(), 0, String::new()));
            }
            Event::TaskListMarker(done) => self.push(if done { "[x] " } else { "[ ] " }),
            Event::Html(_) | Event::InlineHtml(_) | Event::FootnoteReference(_) => {}
            Event::InlineMath(text) | Event::DisplayMath(text) => self.push(&text),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                // A list item's first paragraph is the item.
                if self.kind != "li" || !self.text.trim().is_empty() {
                    self.open(if self.quotes > 0 { "quote" } else { "p" }, String::new());
                }
            }
            Tag::Heading { level, .. } => {
                let kind = match level {
                    HeadingLevel::H1 => "h1",
                    HeadingLevel::H2 => "h2",
                    HeadingLevel::H3 => "h3",
                    HeadingLevel::H4 => "h4",
                    HeadingLevel::H5 => "h5",
                    HeadingLevel::H6 => "h6",
                };
                self.open(kind, String::new());
            }
            Tag::BlockQuote(_) => {
                self.close();
                self.quotes += 1;
            }
            Tag::CodeBlock(_) => {
                self.open("pre", String::new());
                self.in_code = true;
            }
            Tag::List(start) => {
                self.close();
                self.lists.push(start);
            }
            Tag::Item => {
                let marker = match self.lists.last_mut() {
                    Some(Some(next)) => {
                        let marker = format!("{next}.");
                        *next += 1;
                        marker
                    }
                    _ => "•".to_string(),
                };
                self.open("li", marker);
            }
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                self.link = Some((dest_url.to_string(), self.text.len()));
            }
            Tag::TableRow | Tag::TableHead => {
                self.close();
                self.cells.clear();
            }
            Tag::TableCell => self.text.clear(),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item => self.close(),
            TagEnd::CodeBlock => {
                self.in_code = false;
                self.close();
            }
            TagEnd::BlockQuote(_) => {
                self.close();
                self.quotes = self.quotes.saturating_sub(1);
            }
            TagEnd::List(_) => {
                self.close();
                self.lists.pop();
            }
            TagEnd::Link | TagEnd::Image => {
                if let Some((url, start)) = self.link.take() {
                    let text = self.text.get(start..).unwrap_or("").trim().to_string();
                    self.links.push(HtmlLink {
                        text: clean_text(&text),
                        url: clean_text(&url),
                    });
                    self.text.push_str(&format!("[{}]", self.links.len()));
                }
            }
            TagEnd::TableCell => self.cells.push(std::mem::take(&mut self.text).trim().to_string()),
            TagEnd::TableRow | TagEnd::TableHead => {
                let row = std::mem::take(&mut self.cells).join(" │ ");
                self.blocks.push(block("row", clean_text(&row), 0, String::new()));
            }
            _ => {}
        }
    }

    fn push(&mut self, text: &str) {
        if self.kind.is_empty() && self.cells.is_empty() && self.link.is_none() && !text.trim().is_empty() {
            self.open(if self.quotes > 0 { "quote" } else { "p" }, String::new());
        }
        self.text.push_str(text);
    }

    fn open(&mut self, kind: &str, marker: String) {
        self.close();
        self.kind = kind.to_string();
        self.marker = marker;
        self.depth = u8::try_from(self.lists.len()).unwrap_or(u8::MAX).max(self.quotes);
    }

    fn close(&mut self) {
        if self.kind.is_empty() {
            return;
        }
        let kind = std::mem::take(&mut self.kind);
        let text = std::mem::take(&mut self.text);
        let text = if kind == "pre" {
            text.trim_end_matches('\n').to_string()
        } else {
            text.trim().to_string()
        };
        if !text.is_empty() || kind == "li" {
            self.blocks.push(block(
                &kind,
                clean_text(&text),
                self.depth,
                std::mem::take(&mut self.marker),
            ));
        }
        self.marker.clear();
    }
}

fn block(kind: &str, text: String, depth: u8, marker: String) -> HtmlBlock {
    HtmlBlock {
        kind: kind.to_string(),
        text,
        depth,
        marker,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_lists_and_links_read_as_the_html_conversion_writes_them() {
        let (blocks, links) = blocks("# Title\n\nSome *text* with [a link](https://x.example).\n\n- one\n- two\n\n1. first\n\n> quoted\n\n```\ncode\n```\n\n---\n");
        let kinds: Vec<(&str, &str, &str)> = blocks
            .iter()
            .map(|b| (b.kind.as_str(), b.text.as_str(), b.marker.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [
                ("h1", "Title", ""),
                ("p", "Some text with a link[1].", ""),
                ("li", "one", "•"),
                ("li", "two", "•"),
                ("li", "first", "1."),
                ("quote", "quoted", ""),
                ("pre", "code", ""),
                ("rule", "", ""),
            ]
        );
        assert_eq!(links[0].url, "https://x.example");
        assert_eq!(links[0].text, "a link");
    }
}

/// How a character of Markdown source is drawn in the styled-source editor (3.8): the text stays the value, and each
/// character takes the look of what it is part of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceLook {
    pub heading: bool,
    pub strong: bool,
    pub emphasis: bool,
    pub code: bool,
    pub link: bool,
    pub quote: bool,
    /// A heading's `#`, emphasis stars, code ticks, a list bullet, a link's brackets and url: drawn muted.
    pub marker: bool,
}

/// The look of each character of `source`, one per `char`, from pulldown-cmark's source offsets. What it does not
/// read stays plain.
#[cfg_attr(not(feature = "ui"), allow(dead_code))] // the editor of the ui is its caller
pub fn source_looks(source: &str) -> Vec<SourceLook> {
    let mut by_byte = vec![SourceLook::default(); source.len()];
    let mut mark = |range: std::ops::Range<usize>, set: &dyn Fn(&mut SourceLook)| {
        for look in by_byte.get_mut(range).into_iter().flatten() {
            set(look);
        }
    };
    let bytes = source.as_bytes();
    // The run of `ch` at either end of `range`, as markers.
    let ends = |range: &std::ops::Range<usize>, ch: u8| {
        let lead = bytes[range.clone()].iter().take_while(|byte| **byte == ch).count();
        let tail = bytes[range.clone()]
            .iter()
            .rev()
            .take_while(|byte| **byte == ch)
            .count();
        (
            range.start..range.start + lead,
            range.end - tail.min(range.len() - lead)..range.end,
        )
    };
    for (event, range) in
        Parser::new_ext(source, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH).into_offset_iter()
    {
        match event {
            Event::Start(Tag::Heading { .. }) => {
                mark(range.clone(), &|look| look.heading = true);
                let hashes = bytes[range.clone()].iter().take_while(|byte| **byte == b'#').count();
                mark(range.start..range.start + hashes, &|look| look.marker = true);
            }
            Event::Start(Tag::Strong) => {
                mark(range.clone(), &|look| look.strong = true);
                let marker = if bytes.get(range.start) == Some(&b'_') {
                    b'_'
                } else {
                    b'*'
                };
                let (lead, tail) = ends(&range, marker);
                mark(lead, &|look| look.marker = true);
                mark(tail, &|look| look.marker = true);
            }
            Event::Start(Tag::Emphasis) => {
                mark(range.clone(), &|look| look.emphasis = true);
                let marker = if bytes.get(range.start) == Some(&b'_') {
                    b'_'
                } else {
                    b'*'
                };
                let (lead, tail) = ends(&range, marker);
                mark(lead, &|look| look.marker = true);
                mark(tail, &|look| look.marker = true);
            }
            Event::Code(_) => {
                mark(range.clone(), &|look| look.code = true);
                let (lead, tail) = ends(&range, b'`');
                mark(lead, &|look| look.marker = true);
                mark(tail, &|look| look.marker = true);
            }
            Event::Start(Tag::CodeBlock(_)) => mark(range, &|look| look.code = true),
            Event::Start(Tag::Link { .. }) => {
                mark(range.clone(), &|look| look.link = true);
                // `[text](url)`: the brackets and the url are markers, the text is the link.
                if bytes.get(range.start) == Some(&b'[') {
                    mark(range.start..range.start + 1, &|look| look.marker = true);
                    if let Some(close) = source[range.clone()].find("](") {
                        mark(range.start + close..range.end, &|look| look.marker = true);
                    }
                }
            }
            Event::Start(Tag::BlockQuote(_)) => mark(range, &|look| look.quote = true),
            Event::Start(Tag::Item) => {
                // The bullet or number and the space after it.
                let line = &source[range.clone()];
                let marker = line
                    .find(|ch: char| !(ch.is_ascii_digit() || matches!(ch, '-' | '*' | '+' | '.' | ')')))
                    .unwrap_or(0);
                mark(range.start..range.start + marker, &|look| look.marker = true);
            }
            _ => {}
        }
    }
    source.char_indices().map(|(at, _)| by_byte[at]).collect()
}

/// Alt+B, Alt+I, Alt+K in a Markdown text area: the word at the cursor in `**`, `_` or `[…]()`, or an empty pair with
/// the cursor between. The cursor is a char index; the answer is the new text and cursor.
#[cfg_attr(not(feature = "ui"), allow(dead_code))] // the editor of the ui is its caller
pub fn wrap_word(text: &str, cursor: usize, kind: char) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let word = |ch: &char| !ch.is_whitespace();
    let start = chars[..cursor].iter().rposition(|ch| !word(ch)).map_or(0, |at| at + 1);
    let end = chars[cursor..]
        .iter()
        .position(|ch| !word(ch))
        .map_or(chars.len(), |at| cursor + at);
    let (open, close) = match kind {
        'b' => ("**", "**"),
        'i' => ("_", "_"),
        _ => ("[", "]()"),
    };
    let inner: String = chars[start..end].iter().collect();
    let before: String = chars[..start].iter().collect();
    let after: String = chars[end..].iter().collect();
    let text = format!("{before}{open}{inner}{close}{after}");
    let open_len = open.chars().count();
    let cursor = if inner.is_empty() {
        start + open_len
    } else if kind == 'k' {
        // Into the url's parentheses.
        end + open_len + 2
    } else {
        end + open_len + close.chars().count()
    };
    (text, cursor)
}

#[cfg(test)]
mod source_tests {
    use super::*;

    #[test]
    fn source_looks_mark_markers_and_text() {
        let text = "# Title\n\nSome **bold** and `code`, [a link](https://moqui.org).\n\n- item";
        let looks = source_looks(text);
        let at = |needle: &str| text[..text.find(needle).unwrap()].chars().count();
        assert!(looks[0].heading && looks[0].marker);
        assert!(looks[at("Title")].heading && !looks[at("Title")].marker);
        assert!(looks[at("**bold")].marker && looks[at("bold**")].strong && !looks[at("bold**")].marker);
        assert!(looks[at("`code")].marker && looks[at("code`")].code);
        assert!(looks[at("[a link")].marker && looks[at("a link")].link && !looks[at("a link")].marker);
        assert!(looks[at("https")].marker);
        assert!(looks[at("- item")].marker && !looks[at("item")].marker);
        assert_eq!(looks.len(), text.chars().count());
    }

    #[test]
    fn a_word_is_wrapped_or_a_pair_inserted() {
        assert_eq!(
            wrap_word("make it bold now", 9, 'b'),
            ("make it **bold** now".to_string(), 16)
        );
        assert_eq!(wrap_word("an idea", 3, 'i'), ("an _idea_ now".replace(" now", ""), 9));
        assert_eq!(wrap_word("see docs", 5, 'k'), ("see [docs]()".to_string(), 11));
        assert_eq!(wrap_word("end ", 4, 'b'), ("end ****".to_string(), 6));
    }
}
