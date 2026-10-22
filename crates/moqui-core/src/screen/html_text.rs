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

//! HTML as the blocks of text a terminal draws: the `html` node of the tui mode (plans/archive/Screen-TUI-Impl.md 2.3).
//!
//! Headings, paragraphs, list items, quotes, preformatted text and table rows become blocks; everything else is
//! inline text in the block around it. A link's text is followed by `[n]`, and its URL is the n-th entry of `links`,
//! as Lynx numbers the References at the end of a page. Script, style and other content a page does not show is
//! dropped. Control characters are stripped (D9), so a value cannot carry a terminal sequence.

use std::cell::RefCell;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts};

use super::sri::strip_controls;

/// One block of text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// `p`, `h1`…`h6`, `li`, `quote`, `pre`, `row` (a table row, its cells joined by ` | `) or `rule`.
    pub kind: &'static str,
    pub text: String,
    /// Nesting: of lists for `li`, of quotes for `quote` and the blocks inside one.
    pub depth: u8,
    /// The marker of a list item: `•`, or `1.` in an ordered list.
    pub marker: String,
}

/// A link of the text, numbered from 1 in the order it appears.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkRef {
    pub text: String,
    pub url: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HtmlText {
    pub blocks: Vec<Block>,
    pub links: Vec<LinkRef>,
}

impl HtmlText {
    /// The JSON of the `html` node, less its `t` and `id`.
    pub fn to_json(&self) -> serde_json::Value {
        let blocks: Vec<serde_json::Value> = self
            .blocks
            .iter()
            .map(|block| {
                let mut value = serde_json::json!({"kind": block.kind, "text": block.text});
                if block.depth > 0 {
                    value["depth"] = block.depth.into();
                }
                if !block.marker.is_empty() {
                    value["marker"] = block.marker.clone().into();
                }
                value
            })
            .collect();
        let links: Vec<serde_json::Value> = self
            .links
            .iter()
            .map(|link| serde_json::json!({"text": link.text, "url": link.url}))
            .collect();
        serde_json::json!({"blocks": blocks, "links": links})
    }
}

/// Convert HTML to blocks and numbered links.
pub fn html_text(html: &str) -> HtmlText {
    let sink = Sink::default();
    let tokenizer = Tokenizer::new(sink, TokenizerOpts::default());
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(html));
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    let mut state = tokenizer.sink.state.into_inner();
    state.finish_block();
    state.out
}

/// A `<br>` in the text being filled. Input controls are stripped first, so text never holds this one.
const BREAK: char = '\u{1}';

/// Elements whose content a page does not show.
const HIDDEN: [&str; 7] = ["script", "style", "head", "title", "template", "noscript", "iframe"];

#[derive(Default)]
struct Sink {
    state: RefCell<State>,
}

#[derive(Default)]
struct State {
    out: HtmlText,
    /// The block being filled: its kind, marker and text so far.
    kind: Option<&'static str>,
    marker: String,
    text: String,
    /// Open lists, true for an ordered one, with the next number.
    lists: Vec<(bool, u32)>,
    quotes: u8,
    pre: u32,
    hidden: u32,
    /// The text of each open link, and its URL.
    link: Option<(String, String)>,
    cell: usize,
}

impl TokenSink for Sink {
    type Handle = ();

    fn process_token(&self, token: Token, _line_number: u64) -> TokenSinkResult<()> {
        let mut state = self.state.borrow_mut();
        match token {
            Token::TagToken(tag) => return state.tag(&tag),
            Token::CharacterTokens(text) => state.characters(&text),
            Token::NullCharacterToken
            | Token::CommentToken(_)
            | Token::DoctypeToken(_)
            | Token::EOFToken
            | Token::ParseError(_) => {}
        }
        TokenSinkResult::Continue
    }
}

impl State {
    fn tag(&mut self, tag: &Tag) -> TokenSinkResult<()> {
        let name: &str = &tag.name;
        let start = tag.kind == TagKind::StartTag;
        if HIDDEN.contains(&name) {
            if start && !tag.self_closing {
                self.hidden += 1;
                // The tokenizer alone does not know these hold raw text; a sink must say so.
                return match name {
                    "script" => TokenSinkResult::RawData(RawKind::ScriptData),
                    "style" | "iframe" | "noscript" => TokenSinkResult::RawData(RawKind::Rawtext),
                    "title" => TokenSinkResult::RawData(RawKind::Rcdata),
                    _ => TokenSinkResult::Continue,
                };
            }
            if !start {
                self.hidden = self.hidden.saturating_sub(1);
            }
            return TokenSinkResult::Continue;
        }
        if self.hidden > 0 {
            return TokenSinkResult::Continue;
        }
        match (name, start) {
            (
                "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "aside" | "nav" | "dl" | "dt"
                | "dd" | "figure" | "figcaption" | "address",
                _,
            ) => self.finish_block(),
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => self.open_block(heading(name), String::new()),
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => self.finish_block(),
            ("br", _) => self.text.push(BREAK),
            ("hr", _) => {
                self.finish_block();
                self.out.blocks.push(Block {
                    kind: "rule",
                    text: String::new(),
                    depth: self.quotes,
                    marker: String::new(),
                });
            }
            ("ul" | "ol", true) => {
                self.finish_block();
                self.lists.push((name == "ol", 1));
            }
            ("ul" | "ol", false) => {
                self.finish_block();
                self.lists.pop();
            }
            ("li", true) => {
                let marker = match self.lists.last_mut() {
                    Some((true, next)) => {
                        let marker = format!("{next}.");
                        *next += 1;
                        marker
                    }
                    _ => "•".to_string(),
                };
                self.open_block("li", marker);
            }
            ("li", false) => self.finish_block(),
            ("blockquote", true) => {
                self.finish_block();
                self.quotes = self.quotes.saturating_add(1);
            }
            ("blockquote", false) => {
                self.finish_block();
                self.quotes = self.quotes.saturating_sub(1);
            }
            ("pre", true) => {
                self.open_block("pre", String::new());
                self.pre += 1;
            }
            ("pre", false) => {
                self.finish_block();
                self.pre = self.pre.saturating_sub(1);
            }
            ("tr", true) => {
                self.open_block("row", String::new());
                self.cell = 0;
            }
            ("tr" | "table", false) => self.finish_block(),
            ("td" | "th", true) => {
                if self.kind != Some("row") {
                    self.open_block("row", String::new());
                    self.cell = 0;
                }
                if self.cell > 0 {
                    self.text.push_str(" | ");
                }
                self.cell += 1;
            }
            ("a", true) => {
                let href = attribute(tag, "href");
                self.link = Some((String::new(), href));
            }
            ("a", false) => {
                if let Some((text, url)) = self.link.take() {
                    if !url.is_empty() && !url.starts_with('#') && !url.starts_with("javascript:") {
                        self.out.links.push(LinkRef {
                            text: collapse(&text),
                            url: strip_controls(&url),
                        });
                        self.text.push_str(&format!("[{}]", self.out.links.len()));
                    }
                }
            }
            ("img", true) => {
                let alt = attribute(tag, "alt");
                if !alt.is_empty() {
                    self.characters(&format!("[{alt}]"));
                }
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }

    fn characters(&mut self, text: &str) {
        if self.hidden > 0 {
            return;
        }
        let clean = strip_controls(text);
        if let Some((link_text, _)) = self.link.as_mut() {
            link_text.push_str(&clean);
        }
        self.text.push_str(&clean);
    }

    fn open_block(&mut self, kind: &'static str, marker: String) {
        self.finish_block();
        self.kind = Some(kind);
        self.marker = marker;
    }

    /// End the block being filled. Text outside any block element is a paragraph, or a quote inside a blockquote.
    fn finish_block(&mut self) {
        let raw = std::mem::take(&mut self.text);
        let kind = self.kind.take();
        let marker = std::mem::take(&mut self.marker);
        let text = if kind == Some("pre") || self.pre > 0 {
            raw.replace(BREAK, "\n").trim_matches('\n').to_string()
        } else {
            raw.split(BREAK)
                .map(collapse)
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        };
        if text.is_empty() {
            return;
        }
        let kind = kind.unwrap_or(if self.quotes > 0 { "quote" } else { "p" });
        let depth = match kind {
            "li" => u8::try_from(self.lists.len()).unwrap_or(u8::MAX),
            _ => self.quotes,
        };
        self.out.blocks.push(Block {
            kind,
            text,
            depth,
            marker,
        });
    }
}

fn heading(name: &str) -> &'static str {
    match name {
        "h1" => "h1",
        "h2" => "h2",
        "h3" => "h3",
        "h4" => "h4",
        "h5" => "h5",
        _ => "h6",
    }
}

fn attribute(tag: &Tag, name: &str) -> String {
    tag.attrs
        .iter()
        .find(|attr| &*attr.name.local == name)
        .map(|attr| attr.value.to_string())
        .unwrap_or_default()
}

/// Runs of whitespace as one space, trimmed.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_lists_quotes_and_links() {
        let html = r##"<h2>Title</h2><p>Hello <b>big</b>
            world, see <a href="https://example.com/a">the docs</a>.</p>
            <ul><li>one</li><li>two<ol><li>inner</li></ol></li></ul>
            <blockquote><p>quoted</p></blockquote><pre>  keep
  this</pre><script>alert("<p>x</p>")</script><style>p{}</style>
            <table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>
            <p>esc&amp;ape &lt;b&gt;<br>bad&#x1b;]52;x</p><a href="#top">top</a><img alt="logo">"##;
        let out = html_text(html);
        let kinds: Vec<(&str, &str, u8, &str)> = out
            .blocks
            .iter()
            .map(|block| (block.kind, block.text.as_str(), block.depth, block.marker.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [
                ("h2", "Title", 0, ""),
                ("p", "Hello big world, see the docs[1].", 0, ""),
                ("li", "one", 1, "•"),
                ("li", "two", 1, "•"),
                ("li", "inner", 2, "1."),
                ("quote", "quoted", 1, ""),
                ("pre", "  keep\n  this", 0, ""),
                ("row", "A | B", 0, ""),
                ("row", "1 | 2", 0, ""),
                ("p", "esc&ape <b>\nbad]52;x", 0, ""),
                ("p", "top[logo]", 0, ""),
            ]
        );
        assert_eq!(
            out.links,
            [LinkRef {
                text: "the docs".to_string(),
                url: "https://example.com/a".to_string()
            }]
        );
    }
}
