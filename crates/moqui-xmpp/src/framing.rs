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

//! Reading and writing a client's stream (RFC 6120 §4, RFC 7395).
//!
//! Over TCP the stream is one XML document: [`StreamReader`] feeds the bytes to rxml's parser (which refuses DTDs,
//! entity declarations and processing instructions) and builds each child of `<stream:stream>` with minidom's tree
//! builder, a [`Frame::Element`] per stanza. Whitespace between stanzas (keepalives) is dropped. A stanza longer than
//! the limit ends the stream (`policy-violation`). Over WebSocket each frame is one element ([`parse_frame`]):
//! `<open/>` and `<close/>` of the framing namespace stand for the stream's header and footer.

use std::collections::BTreeMap;

use minidom::tree_builder::TreeBuilder;
use minidom::Element;
use rxml::error::EndOrError;
use rxml::{Parse, RawEvent, RawParser};

use crate::ns;

/// What a client sent.
#[derive(Debug)]
pub enum Frame {
    /// The stream's header (`<stream:stream>`, or `<open>` over WebSocket), with its attributes.
    Open(BTreeMap<String, String>),
    Element(Element),
    /// `</stream:stream>`, or `<close/>`.
    Close,
}

/// Why the stream cannot be read further.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FramingError {
    /// Not well-formed XML, or XML this server does not take (`not-well-formed`).
    NotWellFormed(String),
    /// A stanza over the size limit (`policy-violation`).
    TooLarge,
}

/// The parser of one TCP stream; [`StreamReader::reset`] starts a new document (after STARTTLS and after SASL).
pub struct StreamReader {
    parser: RawParser,
    tree: TreeBuilder,
    opened: bool,
    in_stanza: usize,
    max_stanza: usize,
}

impl StreamReader {
    pub fn new(max_stanza: usize) -> StreamReader {
        StreamReader {
            parser: RawParser::new(),
            tree: TreeBuilder::new(),
            opened: false,
            in_stanza: 0,
            max_stanza,
        }
    }

    pub fn reset(&mut self) {
        self.parser = RawParser::new();
        self.tree = TreeBuilder::new();
        self.opened = false;
        self.in_stanza = 0;
    }

    /// The frames in `data`, which may end in the middle of one (the rest waits for more).
    pub fn feed(&mut self, mut data: &[u8]) -> Result<Vec<Frame>, FramingError> {
        let mut frames = Vec::new();
        if self.tree.depth() > 1 {
            self.in_stanza += data.len();
            if self.in_stanza > self.max_stanza {
                return Err(FramingError::TooLarge);
            }
        }
        loop {
            let event = match self.parser.parse(&mut data, false) {
                Ok(Some(event)) => event,
                Ok(None) => {
                    frames.push(Frame::Close);
                    return Ok(frames);
                }
                Err(EndOrError::NeedMoreData) => return Ok(frames),
                Err(EndOrError::Error(err)) => return Err(FramingError::NotWellFormed(err.to_string())),
            };
            let depth = self.tree.depth();
            let (head_close, foot) = match &event {
                // whitespace keepalives between stanzas
                RawEvent::Text(..) if depth <= 1 => continue,
                RawEvent::ElementHeadClose(..) => (true, false),
                RawEvent::ElementFoot(..) => (false, true),
                _ => (false, false),
            };
            self.tree
                .process_event(event)
                .map_err(|err| FramingError::NotWellFormed(err.to_string()))?;
            if head_close && !self.opened && self.tree.depth() == 1 {
                self.opened = true;
                let header = self.tree.top().map(attributes).unwrap_or_default();
                frames.push(Frame::Open(header));
                continue;
            }
            if foot {
                match self.tree.depth() {
                    0 => {
                        frames.push(Frame::Close);
                        return Ok(frames);
                    }
                    1 => {
                        self.in_stanza = 0;
                        if let Some(stanza) = self.tree.unshift_child() {
                            frames.push(Frame::Element(stanza));
                        }
                    }
                    _ => {}
                }
            }
            if self.tree.depth() == 0 && self.opened {
                frames.push(Frame::Close);
                return Ok(frames);
            }
        }
    }
}

fn attributes(element: &Element) -> BTreeMap<String, String> {
    element
        .attrs()
        .iter()
        .map(|((namespace, name), value)| {
            let key = if namespace.is_empty() {
                name.to_string()
            } else if namespace.as_str() == "http://www.w3.org/XML/1998/namespace" {
                format!("xml:{name}")
            } else {
                format!("{{{}}}{name}", namespace.as_str())
            };
            (key, value.clone())
        })
        .collect()
}

/// One WebSocket frame: `<open/>`, `<close/>` or a stanza.
pub fn parse_frame(text: &str) -> Result<Frame, FramingError> {
    let element: Element = text
        .parse()
        .map_err(|err: minidom::Error| FramingError::NotWellFormed(err.to_string()))?;
    if element.is("open", ns::FRAMING) {
        return Ok(Frame::Open(attributes(&element)));
    }
    if element.is("close", ns::FRAMING) {
        return Ok(Frame::Close);
    }
    Ok(Frame::Element(element))
}

/// An element as text.
pub fn write(element: &Element) -> String {
    let mut out = Vec::new();
    if let Err(err) = element.write_to(&mut out) {
        tracing::error!("XMPP: an element could not be written: {err}");
    }
    String::from_utf8(out).unwrap_or_default()
}

/// Text with the XML special characters escaped, for an attribute value.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// The stream header the server sends over TCP.
pub fn stream_header(from: &str, id: &str, to: Option<&str>) -> String {
    let to = to.map(|to| format!(" to='{}'", escape(to))).unwrap_or_default();
    format!(
        "<?xml version='1.0'?><stream:stream xmlns='{}' xmlns:stream='{}' from='{}' id='{}'{to} version='1.0' xml:lang='en'>",
        ns::CLIENT,
        ns::STREAM,
        escape(from),
        escape(id)
    )
}

/// The `<open/>` the server sends over WebSocket.
pub fn open_frame(from: &str, id: &str) -> String {
    format!(
        "<open xmlns='{}' from='{}' id='{}' version='1.0' xml:lang='en'/>",
        ns::FRAMING,
        escape(from),
        escape(id)
    )
}

pub const STREAM_FOOTER: &str = "</stream:stream>";
pub const CLOSE_FRAME: &str = "<close xmlns='urn:ietf:params:xml:ns:xmpp-framing'/>";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_is_read_in_pieces_and_stanzas_come_out_whole() {
        let mut reader = StreamReader::new(1024);
        let text = "<?xml version='1.0'?><stream:stream xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams' \
            to='moqui.local' version='1.0'> <message to='a@b'><body>hi &amp; bye</body></message>\n \
            <iq type='get' id='1'><ping xmlns='urn:xmpp:ping'/></iq></stream:stream>";
        let mut frames = Vec::new();
        for chunk in text.as_bytes().chunks(7) {
            frames.extend(reader.feed(chunk).unwrap());
        }
        assert!(matches!(&frames[0], Frame::Open(attrs) if attrs.get("to").map(String::as_str) == Some("moqui.local")));
        let Frame::Element(message) = &frames[1] else {
            panic!("a stanza")
        };
        assert!(message.is("message", ns::CLIENT));
        assert_eq!(message.get_child("body", ns::CLIENT).unwrap().text(), "hi & bye");
        let Frame::Element(iq) = &frames[2] else {
            panic!("a stanza")
        };
        assert!(iq.has_child("ping", ns::PING));
        assert!(matches!(frames[3], Frame::Close));
        assert_eq!(frames.len(), 4);
    }

    #[test]
    fn entities_and_oversized_stanzas_are_refused() {
        let mut reader = StreamReader::new(1024);
        let refused = reader.feed(b"<?xml version='1.0'?><!DOCTYPE x [<!ENTITY a 'b'>]><x/>");
        assert!(matches!(refused, Err(FramingError::NotWellFormed(_))));
        let mut reader = StreamReader::new(64);
        reader
            .feed(b"<stream:stream xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams'><message>")
            .unwrap();
        assert_eq!(reader.feed(&[b'a'; 100]).err(), Some(FramingError::TooLarge));
    }

    #[test]
    fn websocket_frames_are_whole_elements() {
        let open =
            parse_frame("<open xmlns='urn:ietf:params:xml:ns:xmpp-framing' to='moqui.local' version='1.0'/>").unwrap();
        assert!(matches!(open, Frame::Open(attrs) if attrs["to"] == "moqui.local"));
        assert!(matches!(parse_frame(CLOSE_FRAME).unwrap(), Frame::Close));
        let Frame::Element(presence) = parse_frame("<presence xmlns='jabber:client'/>").unwrap() else {
            panic!("a stanza")
        };
        assert!(presence.is("presence", ns::CLIENT));
        assert!(write(&presence).contains("jabber:client"));
    }
}
