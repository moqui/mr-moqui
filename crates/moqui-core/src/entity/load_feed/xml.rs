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

//! Entity XML data files, read one top-level element at a time.

use std::io::BufRead;

use moqui_lib::error::Error;
use moqui_lib::node::MNode;

use super::loader::{
    apply_default_parameters, apply_default_values, call_data_file_service, parse_flat_top, parse_top, skip_type,
    store_record, Top,
};
use super::pull::Pull;
use super::LoadOptions;
use crate::entity::facade::EntityFacade;

pub(super) fn load_xml_text(
    facade: &EntityFacade,
    location: &str,
    text: &str,
    options: &LoadOptions,
) -> Result<u64, Error> {
    load_xml_reader(facade, location, std::io::Cursor::new(text.as_bytes()), options)
}

/// A document whose root is not `entity-facade-xml` or `seed-data`, such as an `<entities>` definition file. As Java's
/// SAX handler, only the `<seed-data>` (type `seed`) and `<entity-facade-xml>` (its `type`) elements in it are data;
/// everything else is ignored.
fn load_embedded_document<R: BufRead>(
    facade: &EntityFacade,
    location: &str,
    input: &mut Pull<R>,
    mark: usize,
    empty: bool,
    options: &LoadOptions,
) -> Result<u64, Error> {
    if empty {
        return Ok(0);
    }
    input.read_to_end()?;
    let document = input.text_from(mark)?;
    let root = MNode::parse_text_raw(location, &document)?;
    let mut blocks = Vec::new();
    collect_data_blocks(&root, &mut blocks);
    let mut total = 0_u64;
    for block in blocks {
        let file_type = if block.name() == "seed-data" {
            Some("seed".to_string())
        } else {
            block.attribute("type").filter(|value| !value.is_empty())
        };
        if skip_type(file_type.as_deref(), &options.data_types) {
            continue;
        }
        for child in block.children() {
            if is_data_root(&child.name()) {
                continue;
            }
            match parse_top(facade, location, &child)? {
                Some(Top::Record(mut record)) => {
                    apply_default_values(facade, options, &mut record);
                    total = total.saturating_add(store_record(facade, options, record)?);
                }
                Some(Top::Service(mut service)) => {
                    apply_default_parameters(options, &mut service);
                    call_data_file_service(facade, options, service)?;
                }
                None => {}
            }
        }
    }
    Ok(total)
}

fn is_data_root(name: &str) -> bool {
    name == "seed-data" || name == "entity-facade-xml"
}

/// The data elements under `node`, outermost first; one inside another is not read again.
fn collect_data_blocks(node: &MNode, out: &mut Vec<MNode>) {
    for child in node.children() {
        if is_data_root(&child.name()) {
            out.push(child);
        } else {
            collect_data_blocks(&child, out);
        }
    }
}

pub(super) fn load_xml_reader(
    facade: &EntityFacade,
    location: &str,
    reader: impl BufRead,
    options: &LoadOptions,
) -> Result<u64, Error> {
    let mut input = Pull::new(reader);
    input.skip_bom()?;
    input.consume_match(xml_misc, "xml prologue")?;
    // Keep the root bytes. `advance` would drop them, and an `<entities>` file is parsed whole.
    input.hold = true;
    let mark = input.pos;
    let start = input.take_with(xml_start_tag, "xml root")?;
    if !is_data_root(&start.name) {
        // Entity definition files carry seed data under `<entities>`; any other root is read the same way.
        return load_embedded_document(facade, location, &mut input, mark, start.empty, options);
    }
    input.release_hold();
    let file_type = if start.name == "seed-data" {
        Some("seed".to_string())
    } else {
        start.attr("type").filter(|value| !value.is_empty())
    };
    if skip_type(file_type.as_deref(), &options.data_types) {
        return Ok(0);
    }
    if start.empty {
        return Ok(0);
    }
    let mut total = 0_u64;
    loop {
        input.consume_match(xml_misc, "xml prologue")?;
        if input.starts_with("</")? {
            input.consume_match(xml_close_tag, "xml end tag")?;
            break;
        }
        if input.exhausted()? {
            return Err(Error::new(format!("{location} ended before the root was closed")));
        }
        let element = input.take_text(xml_element_end, "xml element")?;
        let top = match parse_flat_top(facade, location, &element)? {
            Some(top) => Some(top),
            None => parse_top(facade, location, &MNode::parse_text_raw(location, &element)?)?,
        };
        match top {
            Some(Top::Record(mut record)) => {
                apply_default_values(facade, options, &mut record);
                total = total.saturating_add(store_record(facade, options, record)?);
            }
            Some(Top::Service(mut service)) => {
                apply_default_parameters(options, &mut service);
                call_data_file_service(facade, options, service)?;
            }
            None => {}
        }
    }
    input.consume_match(xml_misc, "xml prologue")?;
    if !input.exhausted()? {
        return Err(Error::new(format!("{location} has trailing content after the root")));
    }
    Ok(total)
}

struct Start {
    name: String,
    empty: bool,
    attrs: Vec<(String, String)>,
}

impl Start {
    fn attr(&self, name: &str) -> Option<String> {
        self.attrs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    }
}

fn xml_misc(text: &str, eof: bool) -> Result<Option<usize>, Error> {
    if text.is_empty() {
        return if eof { Ok(Some(0)) } else { Ok(None) };
    }
    let bytes = text.as_bytes();
    let mut i = 0;
    loop {
        let next = skip_xml_ws(text, i);
        if next >= bytes.len() {
            return if eof { Ok(Some(next)) } else { Ok(None) };
        }
        i = next;
        if bytes[i] != b'<' {
            return Ok(Some(i));
        }
        let rest = &bytes[i..];
        if rest.starts_with(b"<!--") {
            match find_marker(rest, b"-->") {
                Some(n) => i += n,
                None if eof => return Err(Error::new("unclosed comment")),
                None => return Ok(None),
            }
            continue;
        }
        if rest.starts_with(b"<?") {
            match find_marker(rest, b"?>") {
                Some(n) => i += n,
                None if eof => return Err(Error::new("unclosed processing instruction")),
                None => return Ok(None),
            }
            continue;
        }
        if doctype_prefix(rest) {
            match skip_doctype(rest, eof)? {
                Some(n) => i += n,
                None => return Ok(None),
            }
            continue;
        }
        if misc_prefix(rest) {
            return if eof { Ok(Some(i)) } else { Ok(None) };
        }
        return Ok(Some(i));
    }
}

fn xml_start_tag(text: &str, eof: bool) -> Result<Option<(usize, Start)>, Error> {
    if text.is_empty() {
        return if eof {
            Err(Error::new("xml root ended early"))
        } else {
            Ok(None)
        };
    }
    let bytes = text.as_bytes();
    if bytes[0] != b'<' {
        return Err(Error::new("xml root does not start with '<'"));
    }
    if bytes.len() == 1 {
        return if eof {
            Err(Error::new("xml root ended early"))
        } else {
            Ok(None)
        };
    }
    if matches!(bytes[1], b'/' | b'!' | b'?') {
        return Err(Error::new("xml root is not an element"));
    }
    let Some((name, mut i)) = read_xml_name(text, 1, eof)? else {
        return Ok(None);
    };
    let mut attrs = Vec::new();
    loop {
        let Some(next) = skip_ws_index(text, i, eof)? else {
            return Ok(None);
        };
        i = next;
        if i >= bytes.len() {
            return if eof {
                Err(Error::new("unclosed start tag"))
            } else {
                Ok(None)
            };
        }
        if bytes[i] == b'/' {
            if i + 1 >= bytes.len() {
                return if eof {
                    Err(Error::new("unclosed start tag"))
                } else {
                    Ok(None)
                };
            }
            if bytes[i + 1] != b'>' {
                return Err(Error::new("xml start tag is not closed"));
            }
            return Ok(Some((
                i + 2,
                Start {
                    name,
                    empty: true,
                    attrs,
                },
            )));
        }
        if bytes[i] == b'>' {
            return Ok(Some((
                i + 1,
                Start {
                    name,
                    empty: false,
                    attrs,
                },
            )));
        }
        let Some((attr_name, next)) = read_xml_name(text, i, eof)? else {
            return Ok(None);
        };
        i = next;
        let Some(next) = skip_ws_index(text, i, eof)? else {
            return Ok(None);
        };
        i = next;
        if i >= bytes.len() {
            return if eof {
                Err(Error::new("attribute value is missing"))
            } else {
                Ok(None)
            };
        }
        if bytes[i] != b'=' {
            return Err(Error::new(format!("attribute {attr_name} has no value")));
        }
        i += 1;
        let Some(next) = skip_ws_index(text, i, eof)? else {
            return Ok(None);
        };
        i = next;
        let Some((value, next)) = read_attr_value(text, i, eof)? else {
            return Ok(None);
        };
        i = next;
        attrs.push((attr_name, value));
    }
}

fn xml_close_tag(text: &str, eof: bool) -> Result<Option<usize>, Error> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return if eof {
            Err(Error::new("xml end tag ended early"))
        } else {
            Ok(None)
        };
    }
    if !bytes.starts_with(b"</") {
        return Err(Error::new("xml end tag does not start with '</'"));
    }
    if bytes.len() < 3 {
        return if eof {
            Err(Error::new("xml end tag ended early"))
        } else {
            Ok(None)
        };
    }
    let Some((_, mut i)) = read_xml_name(text, 2, eof)? else {
        return Ok(None);
    };
    let Some(next) = skip_ws_index(text, i, eof)? else {
        return Ok(None);
    };
    i = next;
    if i >= bytes.len() {
        return if eof {
            Err(Error::new("xml end tag is not closed"))
        } else {
            Ok(None)
        };
    }
    if bytes[i] != b'>' {
        return Err(Error::new("xml end tag is not closed"));
    }
    Ok(Some(i + 1))
}

pub(super) fn xml_element_end(text: &str, eof: bool) -> Result<Option<usize>, Error> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return if eof {
            Err(Error::new("xml element ended early"))
        } else {
            Ok(None)
        };
    }
    if bytes[0] != b'<' {
        return Err(Error::new("xml element does not start with '<'"));
    }
    let mut i = 0;
    let mut depth = 0_i32;
    loop {
        if i >= bytes.len() {
            return if eof {
                Err(Error::new("xml element ended early"))
            } else {
                Ok(None)
            };
        }
        if bytes[i] != b'<' {
            if depth == 0 {
                return Err(Error::new("xml element does not start with '<'"));
            }
            i += 1;
            continue;
        }
        match skip_special(&bytes[i..], eof)? {
            Special::Pending => return Ok(None),
            Special::Comment(n) | Special::Cdata(n) | Special::Pi(n) => {
                i += n;
                continue;
            }
            Special::Decl => return Err(Error::new("this declaration is not allowed")),
            Special::Tag => {}
        }
        let Some((consumed, kind)) = scan_tag(&bytes[i..], eof)? else {
            return if eof {
                Err(Error::new("xml element ended early"))
            } else {
                Ok(None)
            };
        };
        if consumed == 0 {
            return Err(Error::new("xml element ended early"));
        }
        i += consumed;
        match kind {
            TagKind::Empty => {
                if depth == 0 {
                    return Ok(Some(i));
                }
            }
            TagKind::Open => depth += 1,
            TagKind::Close => {
                if depth == 0 {
                    return Err(Error::new("xml end tag does not match"));
                }
                depth -= 1;
                if depth == 0 {
                    return Ok(Some(i));
                }
            }
        }
    }
}

enum TagKind {
    Empty,
    Open,
    Close,
}

enum Special {
    Pending,
    Comment(usize),
    Cdata(usize),
    Pi(usize),
    Decl,
    Tag,
}

fn skip_special(bytes: &[u8], eof: bool) -> Result<Special, Error> {
    if bytes.starts_with(b"<!--") {
        return match find_marker(bytes, b"-->") {
            Some(end) => Ok(Special::Comment(end)),
            None if eof => Err(Error::new("unclosed comment")),
            None => Ok(Special::Pending),
        };
    }
    if bytes.starts_with(b"<![CDATA[") {
        return match find_marker(bytes, b"]]>") {
            Some(end) => Ok(Special::Cdata(end)),
            None if eof => Err(Error::new("unclosed cdata")),
            None => Ok(Special::Pending),
        };
    }
    if bytes.starts_with(b"<?") {
        return match find_marker(bytes, b"?>") {
            Some(end) => Ok(Special::Pi(end)),
            None if eof => Err(Error::new("unclosed processing instruction")),
            None => Ok(Special::Pending),
        };
    }
    if markup_prefix(bytes) {
        return if eof {
            Err(Error::new("xml element ended early"))
        } else {
            Ok(Special::Pending)
        };
    }
    if bytes.starts_with(b"<!") {
        Ok(Special::Decl)
    } else {
        Ok(Special::Tag)
    }
}

fn scan_tag(bytes: &[u8], eof: bool) -> Result<Option<(usize, TagKind)>, Error> {
    if bytes.first() != Some(&b'<') {
        return Err(Error::new("xml element does not start with '<'"));
    }
    if bytes.len() < 2 {
        return if eof {
            Err(Error::new("xml element ended early"))
        } else {
            Ok(None)
        };
    }
    let close = bytes[1] == b'/';
    let mut quote: Option<u8> = None;
    let mut i = 1;
    while i < bytes.len() {
        let byte = bytes[i];
        if let Some(q) = quote {
            if byte == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match byte {
            b'"' | b'\'' => {
                quote = Some(byte);
                i += 1;
            }
            b'/' => {
                if i + 1 >= bytes.len() {
                    return if eof {
                        Err(Error::new("xml element ended early"))
                    } else {
                        Ok(None)
                    };
                }
                if bytes[i + 1] == b'>' {
                    let kind = if close { TagKind::Close } else { TagKind::Empty };
                    return Ok(Some((i + 2, kind)));
                }
                i += 1;
            }
            b'>' => {
                let kind = if close { TagKind::Close } else { TagKind::Open };
                return Ok(Some((i + 1, kind)));
            }
            _ => i += 1,
        }
    }
    if eof {
        Err(Error::new("xml element ended early"))
    } else {
        Ok(None)
    }
}

fn skip_xml_ws(text: &str, mut i: usize) -> usize {
    let bytes = text.as_bytes();
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if bytes[i].is_ascii() || !text.is_char_boundary(i) {
            break;
        }
        let Some(ch) = text[i..].chars().next() else {
            break;
        };
        if !ch.is_whitespace() {
            break;
        }
        i += ch.len_utf8();
    }
    i
}

fn skip_ws_index(text: &str, mut i: usize, eof: bool) -> Result<Option<usize>, Error> {
    while i < text.len() {
        let Some(ch) = text[i..].chars().next() else {
            return Err(Error::new("xml split a character"));
        };
        if !ch.is_whitespace() {
            return Ok(Some(i));
        }
        i += ch.len_utf8();
    }
    if eof {
        Ok(Some(i))
    } else {
        Ok(None)
    }
}

fn read_xml_name(text: &str, start: usize, eof: bool) -> Result<Option<(String, usize)>, Error> {
    if start >= text.len() {
        return if eof {
            Err(Error::new("expected a name"))
        } else {
            Ok(None)
        };
    }
    if !text.is_char_boundary(start) {
        return Err(Error::new("xml split a character"));
    }
    let Some(first) = text[start..].chars().next() else {
        return if eof {
            Err(Error::new("expected a name"))
        } else {
            Ok(None)
        };
    };
    if !xml_name_start(first) {
        return Err(Error::new(format!("invalid name start '{first}'")));
    }
    let mut end = start + first.len_utf8();
    while end < text.len() {
        let Some(ch) = text[end..].chars().next() else {
            break;
        };
        if !xml_name_char(ch) {
            break;
        }
        end += ch.len_utf8();
    }
    if end == text.len() && !eof {
        return Ok(None);
    }
    Ok(Some((text[start..end].to_string(), end)))
}

fn read_attr_value(text: &str, i: usize, eof: bool) -> Result<Option<(String, usize)>, Error> {
    if i >= text.len() {
        return if eof {
            Err(Error::new("attribute value is missing"))
        } else {
            Ok(None)
        };
    }
    let quote = text.as_bytes()[i];
    if quote != b'"' && quote != b'\'' {
        return Err(Error::new("attribute value must be quoted"));
    }
    let mut j = i + 1;
    let mut raw = String::new();
    while j < text.len() {
        let Some(ch) = text[j..].chars().next() else {
            return Err(Error::new("xml split a character"));
        };
        if ch == char::from(quote) {
            return Ok(Some((decode_xml_entities(&raw)?, j + 1)));
        }
        if ch == '<' {
            return Err(Error::new("'<' is not allowed in an attribute value"));
        }
        raw.push(ch);
        j += ch.len_utf8();
    }
    if eof {
        Err(Error::new("unclosed attribute value"))
    } else {
        Ok(None)
    }
}

fn decode_xml_entities(raw: &str) -> Result<String, Error> {
    if !raw.contains('&') {
        return Ok(raw.to_string());
    }
    let mut out = String::new();
    let mut rest = raw;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at + 1..];
        let Some(end) = rest.find(';') else {
            return Err(Error::new(format!("unterminated entity '&{rest}'")));
        };
        let body = &rest[..end];
        let ch = decode_xml_entity(body).ok_or_else(|| Error::new(format!("unknown entity '&{body};'")))?;
        out.push(ch);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn decode_xml_entity(body: &str) -> Option<char> {
    match body {
        "lt" => Some('<'),
        "gt" => Some('>'),
        "amp" => Some('&'),
        "apos" => Some('\''),
        "quot" => Some('"'),
        _ if body.starts_with("#x") || body.starts_with("#X") => {
            u32::from_str_radix(body.get(2..)?, 16).ok().and_then(char::from_u32)
        }
        _ if body.starts_with('#') => body.get(1..)?.parse::<u32>().ok().and_then(char::from_u32),
        _ => None,
    }
}

fn xml_name_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_' || ch == ':' || (!ch.is_ascii() && ch.is_alphabetic())
}

fn xml_name_char(ch: char) -> bool {
    xml_name_start(ch) || ch.is_ascii_digit() || ch == '-' || ch == '.' || (!ch.is_ascii() && ch.is_numeric())
}

fn doctype_prefix(bytes: &[u8]) -> bool {
    const WORD: &[u8] = b"<!DOCTYPE";
    if bytes.len() >= WORD.len() {
        bytes[..WORD.len()].eq_ignore_ascii_case(WORD)
    } else {
        bytes.len() > 2 && WORD[..bytes.len()].eq_ignore_ascii_case(bytes)
    }
}

fn skip_doctype(bytes: &[u8], eof: bool) -> Result<Option<usize>, Error> {
    let mut i = 0;
    let mut quote: Option<u8> = None;
    let mut brackets = 0_i32;
    while i < bytes.len() {
        let byte = bytes[i];
        if let Some(q) = quote {
            if byte == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match byte {
            b'"' | b'\'' => {
                quote = Some(byte);
                i += 1;
            }
            b'[' => {
                brackets += 1;
                i += 1;
            }
            b']' => {
                if brackets == 0 {
                    return Err(Error::new("DOCTYPE subset is not closed"));
                }
                brackets -= 1;
                i += 1;
            }
            b'>' if brackets == 0 => return Ok(Some(i + 1)),
            _ => i += 1,
        }
    }
    if eof {
        Err(Error::new("unclosed DOCTYPE"))
    } else {
        Ok(None)
    }
}

fn misc_prefix(bytes: &[u8]) -> bool {
    const MARKERS: [&[u8]; 2] = [b"<!--", b"<?"];
    MARKERS
        .iter()
        .any(|marker| bytes.len() < marker.len() && marker.starts_with(bytes))
}

fn markup_prefix(bytes: &[u8]) -> bool {
    const MARKERS: [&[u8]; 3] = [b"<!--", b"<![CDATA[", b"<?"];
    MARKERS
        .iter()
        .any(|marker| bytes.len() < marker.len() && marker.starts_with(bytes))
}

fn find_marker(bytes: &[u8], marker: &[u8]) -> Option<usize> {
    if marker.is_empty() || bytes.len() < marker.len() {
        return None;
    }
    bytes
        .windows(marker.len())
        .position(|window| window == marker)
        .map(|at| at + marker.len())
}
