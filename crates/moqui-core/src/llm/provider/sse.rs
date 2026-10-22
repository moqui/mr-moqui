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

//! Server-sent events (the WHATWG event stream), parsed from bytes as they arrive: lines end in LF, CR or CRLF (a CR at
//! the end of one read and its LF at the start of the next are one line end); a line starting with `:` is a comment;
//! `data:` lines join with LF; `event:` and `id:` name the event; one space after the colon is dropped; a blank line
//! ends the event. A `data: [DONE]` event is [`Frame::is_done`]. One event over [`MAX_EVENT_BYTES`] is an error: a
//! stream that never sends a blank line cannot grow without bound.

/// The largest event, its fields together.
pub const MAX_EVENT_BYTES: usize = 1024 * 1024;

/// One event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    /// The `event:` field; `None` is the default type, `message`.
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
}

impl Frame {
    /// The `[DONE]` sentinel of the OpenAI streams.
    pub fn is_done(&self) -> bool {
        self.data.trim() == "[DONE]"
    }
}

/// The parse state between reads.
#[derive(Debug, Default)]
pub struct SseParser {
    line: Vec<u8>,
    /// The last byte was a CR, so a LF at the start of the next read ends nothing.
    after_cr: bool,
    /// The start of the stream, where a byte-order mark is dropped.
    started: bool,
    event: Option<String>,
    data: String,
    has_data: bool,
    id: Option<String>,
    size: usize,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// The events `bytes` complete, in order.
    ///
    /// # Errors
    ///
    /// Returns an error when an event is larger than [`MAX_EVENT_BYTES`].
    pub fn push(&mut self, mut bytes: &[u8]) -> Result<Vec<Frame>, String> {
        if !self.started && !bytes.is_empty() {
            self.started = true;
            if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
                bytes = rest;
            }
        }
        let mut frames = Vec::new();
        for &byte in bytes {
            match byte {
                b'\n' if self.after_cr => self.after_cr = false,
                b'\n' | b'\r' => {
                    self.after_cr = byte == b'\r';
                    let line = std::mem::take(&mut self.line);
                    if let Some(frame) = self.line_ended(&line) {
                        frames.push(frame);
                    }
                }
                _ => {
                    self.after_cr = false;
                    self.line.push(byte);
                    self.size += 1;
                    if self.size > MAX_EVENT_BYTES {
                        return Err(format!("a stream event is larger than {MAX_EVENT_BYTES} bytes"));
                    }
                }
            }
        }
        Ok(frames)
    }

    /// The end of the stream: an event with no blank line after it still counts.
    pub fn finish(&mut self) -> Option<Frame> {
        if !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            if let Some(frame) = self.line_ended(&line) {
                return Some(frame);
            }
        }
        self.dispatch()
    }

    fn line_ended(&mut self, line: &[u8]) -> Option<Frame> {
        if line.is_empty() {
            return self.dispatch();
        }
        if line[0] == b':' {
            return None;
        }
        let text = String::from_utf8_lossy(line);
        let (field, value) = match text.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (text.as_ref(), ""),
        };
        match field {
            "data" => {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
            }
            "event" => self.event = Some(value.to_string()),
            // an id with a NUL is ignored, as the standard says
            "id" if !value.contains('\0') => self.id = Some(value.to_string()),
            _ => {}
        }
        None
    }

    fn dispatch(&mut self) -> Option<Frame> {
        self.size = 0;
        let event = self.event.take();
        if !self.has_data {
            return None;
        }
        self.has_data = false;
        Some(Frame {
            event: event.filter(|event| !event.is_empty()),
            data: std::mem::take(&mut self.data),
            id: self.id.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(chunks: &[&[u8]]) -> Vec<Frame> {
        let mut parser = SseParser::new();
        let mut frames = Vec::new();
        for chunk in chunks {
            frames.extend(parser.push(chunk).unwrap());
        }
        frames.extend(parser.finish());
        frames
    }

    fn data(text: &str) -> Frame {
        Frame {
            data: text.to_string(),
            ..Frame::default()
        }
    }

    #[test]
    fn events_split_anywhere_with_any_line_end() {
        let stream = b"data: one\r\n\r\n: a comment\n\nevent: delta\ndata: two\rdata:  three\r\rid: 7\ndata: four\n\n";
        let whole = all(&[stream]);
        let expected = vec![
            data("one"),
            Frame {
                event: Some("delta".to_string()),
                data: "two\n three".to_string(),
                id: None,
            },
            Frame {
                event: None,
                data: "four".to_string(),
                id: Some("7".to_string()),
            },
        ];
        assert_eq!(whole, expected);
        // the same bytes one at a time: a CR and its LF in two reads are one line end
        let bytes: Vec<&[u8]> = stream.chunks(1).collect();
        assert_eq!(all(&bytes), expected);
        for at in 1..stream.len() {
            assert_eq!(all(&[&stream[..at], &stream[at..]]), expected, "split at {at}");
        }
    }

    #[test]
    fn done_bom_empty_data_and_an_unterminated_last_event() {
        let frames = all(&[b"\xEF\xBB\xBFdata: {\"a\":1}\n\ndata\n\nevent: ping\n\ndata: [DONE]"]);
        assert_eq!(frames, vec![data("{\"a\":1}"), data(""), data("[DONE]")]);
        assert!(frames[2].is_done() && !frames[0].is_done());
        // a field with no value and an event with no data dispatch nothing
        assert!(all(&[b"event: ping\nretry: 10\n\n"]).is_empty());
        // a multi-byte character split across reads
        let text = "data: caf\u{e9}\n\n".as_bytes();
        assert_eq!(all(&[&text[..9], &text[9..]]), vec![data("caf\u{e9}")]);
    }

    #[test]
    fn an_event_over_the_limit_is_an_error() {
        let mut parser = SseParser::new();
        let big = vec![b'x'; MAX_EVENT_BYTES];
        assert!(parser.push(b"data: ").is_ok());
        let error = parser.push(&big).unwrap_err();
        assert!(error.contains("larger than"), "{error}");
        // lines of one event count together
        let mut parser = SseParser::new();
        let half = vec![b'y'; MAX_EVENT_BYTES / 2];
        parser.push(b"data: ").unwrap();
        parser.push(&half).unwrap();
        parser.push(b"\ndata: ").unwrap();
        assert!(parser.push(&half).is_err());
        // a blank line resets the count
        let mut parser = SseParser::new();
        for _ in 0..3 {
            parser.push(b"data: ").unwrap();
            parser.push(&half).unwrap();
            assert_eq!(parser.push(b"\n\n").unwrap().len(), 1);
        }
    }
}
