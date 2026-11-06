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

//! SVG assets cleaned to an allow-list of drawing elements and attributes (design-shop §12.4): no script, style,
//! foreign content, animation, event attribute or reference outside the image. Validation reports what cleaning would
//! remove for safety, with its line; harmless editor metadata is dropped without a finding.

use std::cell::RefCell;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts};

use super::markup::{self, Policy, UrlKind};

/// Allowed elements, by the lowercase name the tokenizer gives, with the name to write.
const ELEMENTS: &[(&str, &str)] = &[
    ("svg", "svg"),
    ("g", "g"),
    ("defs", "defs"),
    ("symbol", "symbol"),
    ("use", "use"),
    ("path", "path"),
    ("rect", "rect"),
    ("circle", "circle"),
    ("ellipse", "ellipse"),
    ("line", "line"),
    ("polyline", "polyline"),
    ("polygon", "polygon"),
    ("text", "text"),
    ("tspan", "tspan"),
    ("textpath", "textPath"),
    ("title", "title"),
    ("desc", "desc"),
    ("lineargradient", "linearGradient"),
    ("radialgradient", "radialGradient"),
    ("stop", "stop"),
    ("clippath", "clipPath"),
    ("mask", "mask"),
    ("pattern", "pattern"),
    ("marker", "marker"),
    ("image", "image"),
    ("filter", "filter"),
    ("fegaussianblur", "feGaussianBlur"),
    ("feoffset", "feOffset"),
    ("feblend", "feBlend"),
    ("fecolormatrix", "feColorMatrix"),
    ("feflood", "feFlood"),
    ("fecomposite", "feComposite"),
    ("femerge", "feMerge"),
    ("femergenode", "feMergeNode"),
    ("fedropshadow", "feDropShadow"),
];

/// Elements whose removal is a finding, with their content dropped too.
const DANGEROUS: &[&str] = &[
    "script",
    "style",
    "foreignobject",
    "iframe",
    "object",
    "embed",
    "animate",
    "animatemotion",
    "animatetransform",
    "set",
    "handler",
    "listener",
    "a",
];

/// Attributes written in camel case.
const CAMEL: &[(&str, &str)] = &[
    ("viewbox", "viewBox"),
    ("preserveaspectratio", "preserveAspectRatio"),
    ("gradientunits", "gradientUnits"),
    ("gradienttransform", "gradientTransform"),
    ("patternunits", "patternUnits"),
    ("patterncontentunits", "patternContentUnits"),
    ("patterntransform", "patternTransform"),
    ("clippathunits", "clipPathUnits"),
    ("maskunits", "maskUnits"),
    ("maskcontentunits", "maskContentUnits"),
    ("markerwidth", "markerWidth"),
    ("markerheight", "markerHeight"),
    ("markerunits", "markerUnits"),
    ("refx", "refX"),
    ("refy", "refY"),
    ("stddeviation", "stdDeviation"),
    ("filterunits", "filterUnits"),
    ("primitiveunits", "primitiveUnits"),
    ("textlength", "textLength"),
    ("lengthadjust", "lengthAdjust"),
    ("startoffset", "startOffset"),
    ("pathlength", "pathLength"),
    ("spreadmethod", "spreadMethod"),
];

/// Attributes kept as written.
const ATTRIBUTES: &[&str] = &[
    "id",
    "class",
    "x",
    "y",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "fx",
    "fy",
    "d",
    "points",
    "width",
    "height",
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-dasharray",
    "stroke-dashoffset",
    "stroke-opacity",
    "opacity",
    "transform",
    "offset",
    "stop-color",
    "stop-opacity",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "text-anchor",
    "dominant-baseline",
    "letter-spacing",
    "clip-path",
    "clip-rule",
    "mask",
    "marker-start",
    "marker-mid",
    "marker-end",
    "filter",
    "visibility",
    "display",
    "color",
    "xmlns",
    "xmlns:xlink",
    "version",
    "role",
    "aria-label",
    "aria-hidden",
    "aria-labelledby",
    "focusable",
    "href",
    "xlink:href",
    "style",
    "xml:space",
    "in",
    "in2",
    "result",
    "mode",
    "operator",
    "values",
    "type",
    "dx",
    "dy",
    "k1",
    "k2",
    "k3",
    "k4",
    "flood-color",
    "flood-opacity",
];

/// An SVG cleaned to the safe set.
pub fn sanitize_svg(text: &str) -> String {
    clean(text).0
}

/// The cleaned SVG, and each removal made for safety with its line.
pub(crate) fn clean(text: &str) -> (String, Vec<(usize, String)>) {
    let sink = Sink {
        state: RefCell::new(State::default()),
    };
    let tokenizer = Tokenizer::new(sink, TokenizerOpts::default());
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(&markup::strip_reserved(text)));
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    let state = tokenizer.sink.state.into_inner();
    (state.out, state.problems)
}

#[derive(Default)]
struct State {
    out: String,
    problems: Vec<(usize, String)>,
    /// The dropped element being skipped and how deep inside it.
    skip: Option<(String, usize)>,
}

struct Sink {
    state: RefCell<State>,
}

fn escape(text: &str, attribute: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

impl TokenSink for Sink {
    type Handle = ();

    fn process_token(&self, token: Token, line_number: u64) -> TokenSinkResult<()> {
        let line = usize::try_from(line_number).unwrap_or(0);
        let mut state = self.state.borrow_mut();
        match token {
            Token::TagToken(tag) => return state.tag(&tag, line),
            Token::CharacterTokens(text) => {
                if state.skip.is_none() {
                    let escaped = escape(&text, false);
                    state.out.push_str(&escaped);
                }
            }
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
    fn tag(&mut self, tag: &Tag, line: usize) -> TokenSinkResult<()> {
        let name: &str = &tag.name;
        let start = tag.kind == TagKind::StartTag;
        if let Some((skipped, depth)) = &mut self.skip {
            if name == skipped.as_str() {
                if start && !tag.self_closing {
                    *depth += 1;
                } else if !start {
                    *depth -= 1;
                    if *depth == 0 {
                        self.skip = None;
                    }
                }
            }
            return TokenSinkResult::Continue;
        }
        let Some((_, written)) = ELEMENTS.iter().find(|(lower, _)| *lower == name) else {
            if DANGEROUS.contains(&name) {
                if start {
                    self.problems.push((line, format!("<{name}> removed")));
                    if !tag.self_closing {
                        self.skip = Some((name.to_string(), 1));
                        if matches!(name, "script" | "style") {
                            return TokenSinkResult::RawData(if name == "script" {
                                RawKind::ScriptData
                            } else {
                                RawKind::Rawtext
                            });
                        }
                    }
                }
            } else if start && !tag.self_closing && !name.contains(':') {
                // Editor metadata (`metadata`, `sodipodi:namedview`) goes with its content, without a finding.
                self.skip = Some((name.to_string(), 1));
            }
            return TokenSinkResult::Continue;
        };
        if !start {
            self.out.push_str(&format!("</{written}>"));
            return TokenSinkResult::Continue;
        }
        let mut out = format!("<{written}");
        let policy = Policy { origins: &[] };
        for attribute in &tag.attrs {
            let attribute_name: &str = &attribute.name.local;
            let value: &str = &attribute.value;
            let kept = CAMEL
                .iter()
                .find(|(lower, _)| *lower == attribute_name)
                .map(|(_, camel)| *camel)
                .or_else(|| ATTRIBUTES.iter().find(|known| **known == attribute_name).copied());
            let Some(kept) = kept else {
                if attribute_name.starts_with("on") {
                    self.problems
                        .push((line, format!("event attribute `{attribute_name}` on <{name}> removed")));
                }
                continue;
            };
            let unsafe_reason = if matches!(kept, "href" | "xlink:href") {
                let kind = if name == "image" {
                    UrlKind::Resource
                } else {
                    UrlKind::Fragment
                };
                let data_image = name == "image" && value.trim_start().to_ascii_lowercase().starts_with("data:image/");
                match markup::check_url(value, kind, &policy) {
                    Ok(()) if name != "image" || data_image => None,
                    Ok(()) => Some("an image outside the SVG".to_string()),
                    Err(message) => Some(message),
                }
            } else if kept == "style" {
                markup::check_css(value, &policy, None)
                    .first()
                    .map(|found| found.message.clone())
            } else if value.to_ascii_lowercase().contains("url(") {
                let lower = value.to_ascii_lowercase();
                let outside = lower
                    .split("url(")
                    .skip(1)
                    .any(|rest| !rest.trim_start().trim_start_matches(['"', '\'']).starts_with('#'));
                outside.then(|| "url() outside the SVG".to_string())
            } else {
                None
            };
            if let Some(reason) = unsafe_reason {
                self.problems
                    .push((line, format!("`{attribute_name}` on <{name}> removed: {reason}")));
                continue;
            }
            out.push_str(&format!(" {kept}=\"{}\"", escape(value, true)));
        }
        out.push_str(if tag.self_closing { "/>" } else { ">" });
        self.out.push_str(&out);
        TokenSinkResult::Continue
    }
}
