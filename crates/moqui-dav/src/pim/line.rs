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

//! Content lines, shared by iCalendar (RFC 5545) and vCard (RFC 6350, 2426 and 2.1): unfolding, the name, group and
//! parameters of a property, text escaping, quoted-printable values with their character set, and folding at 75
//! octets on write.

/// One property of a content line: `item1.EMAIL;TYPE=work,pref:jane@example.com`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Property {
    /// The group before a dot (Apple's `item1`), if any.
    pub(crate) group: Option<String>,
    /// Upper case.
    pub(crate) name: String,
    /// Parameter names upper case, each with its values unquoted (RFC 6868 caret escapes decoded).
    pub(crate) params: Vec<(String, Vec<String>)>,
    /// The value as written, after a quoted-printable value is decoded in its character set.
    pub(crate) value: String,
}

impl Property {
    /// The first value of a parameter.
    pub(crate) fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .and_then(|(_, values)| values.first())
            .map(String::as_str)
    }

    /// Every value of every parameter of this name, each comma list split (TYPE=home,pref and TYPE=home;TYPE=pref).
    pub(crate) fn param_values<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.params
            .iter()
            .filter(move |(key, _)| key.eq_ignore_ascii_case(name))
            .flat_map(|(_, values)| values.iter())
            .flat_map(|value| value.split(','))
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    /// Whether TYPE has this value, ignoring case.
    pub(crate) fn has_type(&self, kind: &str) -> bool {
        self.param_values("TYPE").any(|value| value.eq_ignore_ascii_case(kind))
    }
}

/// Splits text into logical lines. A line ends with CRLF, LF or CR; a line that starts with a space or a tab
/// continues the line before it (that one character dropped). A vCard 2.1 quoted-printable value that ends with `=`
/// is a soft line break and continues on the next physical line as it is.
pub(crate) fn unfold(text: &str) -> Vec<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines: Vec<String> = Vec::new();
    let mut soft_break = false;
    for physical in text.split('\n') {
        let physical = physical.strip_suffix('\r').unwrap_or(physical);
        // a bare CR line end
        for (index, part) in physical.split('\r').enumerate() {
            let continues = index == 0 && soft_break;
            if continues {
                if let Some(last) = lines.last_mut() {
                    last.push_str(part);
                }
            } else if (part.starts_with(' ') || part.starts_with('\t')) && !lines.is_empty() {
                if let Some(last) = lines.last_mut() {
                    last.push_str(&part[1..]);
                }
            } else {
                lines.push(part.to_string());
            }
            soft_break = lines
                .last()
                .is_some_and(|line| is_quoted_printable(line) && line.ends_with('='));
            if soft_break {
                if let Some(last) = lines.last_mut() {
                    last.pop();
                }
            }
        }
    }
    lines.retain(|line| !line.trim().is_empty());
    lines
}

/// Whether the parameters of a line (before its first colon outside quotes) say quoted-printable.
fn is_quoted_printable(line: &str) -> bool {
    let head = &line[..value_start(line).unwrap_or(line.len())];
    head.to_ascii_uppercase().contains("QUOTED-PRINTABLE")
}

/// The index of the colon that starts the value: the first one outside a quoted parameter value.
fn value_start(line: &str) -> Option<usize> {
    let mut quoted = false;
    for (index, ch) in line.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            ':' if !quoted => return Some(index),
            _ => {}
        }
    }
    None
}

/// Reads one logical line. None for a line without a name or a colon.
pub(crate) fn parse_line(line: &str) -> Option<Property> {
    let colon = value_start(line)?;
    let head = &line[..colon];
    let mut value = line[colon + 1..].to_string();
    let mut parts = split_head(head);
    if parts.is_empty() {
        return None;
    }
    let full_name = parts.remove(0);
    let full_name = full_name.trim();
    if full_name.is_empty() {
        return None;
    }
    let (group, name) = match full_name.rfind('.') {
        Some(dot) => (
            Some(full_name[..dot].to_string()),
            full_name[dot + 1..].to_ascii_uppercase(),
        ),
        None => (None, full_name.to_ascii_uppercase()),
    };
    let mut params: Vec<(String, Vec<String>)> = Vec::new();
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.find('=') {
            Some(eq) => {
                let key = part[..eq].trim().to_ascii_uppercase();
                let values = split_param_values(&part[eq + 1..]);
                params.push((key, values));
            }
            None => {
                // vCard 2.1 writes a bare value: TEL;WORK;VOICE, NOTE;QUOTED-PRINTABLE
                let upper = part.to_ascii_uppercase();
                let key = match upper.as_str() {
                    "QUOTED-PRINTABLE" | "BASE64" | "B" | "8BIT" | "7BIT" => "ENCODING",
                    _ => "TYPE",
                };
                params.push((key.to_string(), vec![part.to_string()]));
            }
        }
    }
    let property = Property {
        group,
        name,
        params,
        value: String::new(),
    };
    if property
        .param("ENCODING")
        .is_some_and(|encoding| encoding.eq_ignore_ascii_case("QUOTED-PRINTABLE"))
    {
        value = decode_charset(&decode_qp(&value), property.param("CHARSET"));
    }
    Some(Property { value, ..property })
}

/// The parts of a line's head split on semicolons outside quotes.
fn split_head(head: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in head.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                current.push(ch);
            }
            ';' if !quoted => parts.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    parts.push(current);
    parts
}

/// A parameter's values: a comma list, each value possibly in double quotes, with RFC 6868 caret escapes.
fn split_param_values(text: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in text.chars() {
        match ch {
            '"' => quoted = !quoted,
            ',' if !quoted => values.push(decode_caret(&std::mem::take(&mut current))),
            _ => current.push(ch),
        }
    }
    values.push(decode_caret(&current));
    values
}

/// RFC 6868: `^n` a line break, `^'` a double quote, `^^` a caret.
fn decode_caret(text: &str) -> String {
    if !text.contains('^') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '^' {
            match chars.peek() {
                Some('n') | Some('N') => {
                    out.push('\n');
                    chars.next();
                }
                Some('\'') => {
                    out.push('"');
                    chars.next();
                }
                Some('^') => {
                    out.push('^');
                    chars.next();
                }
                _ => out.push('^'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Quoted-printable bytes (soft line breaks are already gone).
pub(crate) fn decode_qp(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'=' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex_digit(bytes[index + 1]), hex_digit(bytes[index + 2])) {
                out.push(high << 4 | low);
                index += 3;
                continue;
            }
        }
        out.push(byte);
        index += 1;
    }
    out
}

fn hex_digit(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).map(|digit| digit as u8)
}

/// Bytes in a character set by its label (UTF-8, ISO-8859-1, Windows-1252, ...): UTF-8 when there is none, and
/// Windows-1252 for bytes that are not UTF-8 (Outlook's exports).
pub(crate) fn decode_charset(bytes: &[u8], charset: Option<&str>) -> String {
    let encoding = charset
        .and_then(|label| encoding_rs::Encoding::for_label(label.trim().as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    if encoding == encoding_rs::UTF_8 {
        if let Ok(text) = std::str::from_utf8(bytes) {
            return text.to_string();
        }
        return encoding_rs::WINDOWS_1252.decode(bytes).0.into_owned();
    }
    encoding.decode(bytes).0.into_owned()
}

/// A text value's escapes: `\n` and `\N` line breaks, `\\`, `\,`, `\;` and `\:`.
pub(crate) fn unescape(text: &str) -> String {
    if !text.contains('\\') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Splits on a separator that is not escaped with a backslash; the parts keep their escapes.
pub(crate) fn split_unescaped(text: &str, separator: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for ch in text.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
        } else if ch == '\\' {
            current.push(ch);
            escaped = true;
        } else if ch == separator {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
    }
    parts.push(current);
    parts
}

/// A text value escaped: backslash, comma, semicolon and line breaks.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ',' => out.push_str("\\,"),
            ';' => out.push_str("\\;"),
            '\r' => {
                if chars.peek() != Some(&'\n') {
                    out.push_str("\\n");
                }
            }
            '\n' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out
}

/// A parameter value, in double quotes when it has a colon, a semicolon or a comma (RFC 6868 carets for line breaks
/// and quotes).
pub(crate) fn param_value(text: &str) -> String {
    let mut value = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '^' => value.push_str("^^"),
            '"' => value.push_str("^'"),
            '\n' => value.push_str("^n"),
            '\r' => {}
            _ => value.push(ch),
        }
    }
    if value.contains([':', ';', ',']) {
        format!("\"{value}\"")
    } else {
        value
    }
}

/// A content line folded at 75 octets, never inside a UTF-8 character; the continuation lines start with a space.
pub(crate) fn fold(line: &str) -> String {
    if line.len() <= 75 {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + line.len() / 70 * 3);
    let mut size = 0;
    let mut limit = 75;
    for ch in line.chars() {
        let width = ch.len_utf8();
        if size + width > limit {
            out.push_str("\r\n ");
            size = 0;
            // the leading space counts
            limit = 74;
        }
        out.push(ch);
        size += width;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_unfold_and_read_their_parameters() {
        // one space or tab is dropped from a continuation line, the rest is the value's
        let lines = unfold("BEGIN:VCARD\r\nNOTE:one\r\n  two\r\n\tthree\nitem1.EMAIL;type=INTERNET;TYPE=\"home,pref\":a@b.c\rEND:VCARD\r\n");
        assert_eq!(
            lines,
            [
                "BEGIN:VCARD",
                "NOTE:one twothree",
                "item1.EMAIL;type=INTERNET;TYPE=\"home,pref\":a@b.c",
                "END:VCARD"
            ]
        );
        let email = parse_line(&lines[2]).unwrap();
        assert_eq!(email.group.as_deref(), Some("item1"));
        assert_eq!(email.name, "EMAIL");
        assert!(email.has_type("pref") && email.has_type("HOME") && email.has_type("internet"));
        assert_eq!(email.value, "a@b.c");
        let quoted = parse_line("ATTENDEE;CN=\"Doe, Jane\";DELEGATED-FROM=\"mailto:a@b.c\":mailto:jane@b.c").unwrap();
        assert_eq!(quoted.param("CN"), Some("Doe, Jane"));
        assert_eq!(quoted.param("delegated-from"), Some("mailto:a@b.c"));
        assert_eq!(quoted.value, "mailto:jane@b.c");
        let caret = parse_line("ADR;LABEL=\"1 Main St^nSpringfield ^'Home^'\":;;1 Main St;Springfield;;;").unwrap();
        assert_eq!(caret.param("LABEL"), Some("1 Main St\nSpringfield \"Home\""));
    }

    #[test]
    fn quoted_printable_values_join_soft_breaks_and_read_their_charset() {
        let text = "NOTE;ENCODING=QUOTED-PRINTABLE;CHARSET=UTF-8:Caf=C3=A9 =\r\nau lait=0D=0Asecond line\r\nTEL;WORK;VOICE:555\r\n\
                    N;CHARSET=Windows-1252;QUOTED-PRINTABLE:M=FCller;J=FCrgen\r\n";
        let lines = unfold(text);
        assert_eq!(lines.len(), 3, "{lines:?}");
        let note = parse_line(&lines[0]).unwrap();
        assert_eq!(note.value, "Café au lait\r\nsecond line");
        let tel = parse_line(&lines[1]).unwrap();
        assert!(tel.has_type("work") && tel.has_type("voice"));
        let name = parse_line(&lines[2]).unwrap();
        assert_eq!(name.value, "Müller;Jürgen");
        // bytes that are not UTF-8 are read as Windows-1252
        assert_eq!(
            decode_charset(&decode_qp("=93quoted=94"), None),
            "\u{201c}quoted\u{201d}"
        );
    }

    #[test]
    fn text_escapes_and_folds() {
        assert_eq!(
            unescape("Met at the fair\\, 2024\\nNext line\\; done\\\\"),
            "Met at the fair, 2024\nNext line; done\\"
        );
        assert_eq!(escape("a,b;c\\d\r\ne"), "a\\,b\\;c\\\\d\\ne");
        assert_eq!(split_unescaped("Doe;Jane\\;x;;", ';'), ["Doe", "Jane\\;x", "", ""]);
        assert_eq!(param_value("Doe, Jane"), "\"Doe, Jane\"");
        let long = format!("DESCRIPTION:{}", "é".repeat(60));
        let folded = fold(&long);
        assert!(folded.split("\r\n").all(|line| line.len() <= 75), "{folded}");
        assert_eq!(unfold(&folded), [long]);
    }
}
