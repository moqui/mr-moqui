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

//! String helpers from `StringUtilities`.
//!
//! Groovy class-name cleanup, DOM element text, Elastic query helpers, and
//! `numberToWords` are not ported.

const SCALES: [&str; 8] = [
    "",
    "thousand",
    "million",
    "billion",
    "trillion",
    "quadrillion",
    "quintillion",
    "sextillion",
];
const SUBTWENTY: [&str; 20] = [
    "",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const DECADES: [&str; 10] = [
    "", "ten", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

/// A value from 0 to 999 in words, as Java's `tripleAsWords` writes it, down to its extra space after "hundred".
fn triple_as_words(value: usize, use_and: bool) -> String {
    if value < SUBTWENTY.len() {
        return SUBTWENTY[value].to_string();
    }
    let subhun = value % 100;
    let hun = value / 100;
    let mut out = String::new();
    if hun > 0 {
        out.push_str(SUBTWENTY[hun]);
        out.push_str(" hundred");
    }
    if subhun > 0 {
        if hun > 0 {
            out.push_str(if use_and { " and " } else { " " });
        }
        if subhun < SUBTWENTY.len() {
            out.push(' ');
            out.push_str(SUBTWENTY[subhun]);
        } else {
            let tens = subhun / 10;
            let units = subhun % 10;
            if tens > 0 {
                out.push_str(DECADES[tens]);
            }
            if units > 0 {
                out.push(' ');
                out.push_str(SUBTWENTY[units]);
            }
        }
    }
    out
}

/// `StringUtilities.numberToWords(value, useAnd)`: 0 is the empty text, as Java's (its first word is empty), and the
/// first letter is capital.
pub fn number_to_words(value: i64, use_and: bool) -> String {
    if value == 0 {
        return String::new();
    }
    let negative = value < 0;
    let mut remaining = i128::from(value);
    let mut thousands: Vec<usize> = Vec::new();
    while remaining != 0 {
        thousands.push(usize::try_from((remaining % 1000).abs()).unwrap_or(0));
        remaining /= 1000;
    }
    let mut out = String::new();
    if negative {
        out.push_str("negative ");
    }
    let mut first = true;
    for scale in (1..thousands.len()).rev() {
        if !first {
            out.push_str(", ");
        }
        first = false;
        if thousands[scale] > 0 {
            out.push_str(&triple_as_words(thousands[scale], use_and));
            out.push(' ');
            out.push_str(SCALES[scale]);
        }
    }
    if !first && thousands[0] != 0 {
        out.push_str(if use_and { " and " } else { " " });
    }
    out.push_str(&triple_as_words(thousands[0], use_and));
    let mut chars = out.chars();
    match chars.next() {
        Some(head) => head.to_uppercase().chain(chars).collect(),
        None => out,
    }
}

/// Encode text for an XML attribute. Characters above ASCII become decimal
/// character references. The control-character substitutions match
/// `StringUtilities.encodeForXmlAttribute`.
pub fn encode_for_xml_attribute(original: &str, add_zero_width_spaces: bool) -> String {
    let mut out = String::with_capacity(original.len());
    for ch in original.chars() {
        match ch {
            '\'' => out.push_str("&apos;"),
            '"' => out.push_str("&quot;"),
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\u{0005}' => out.push_str("..."),
            '\u{0012}' => out.push_str("&apos;"),
            '\u{0013}' | '\u{0014}' => out.push_str("&quot;"),
            '\u{0016}' | '\u{0017}' => out.push('-'),
            '\u{0019}' => out.push_str("tm"),
            other => {
                let code = other as u32;
                if code < 0x20 && other != '\t' && other != '\n' && other != '\r' {
                    continue;
                }
                if code > 0x7F {
                    out.push_str("&#");
                    out.push_str(&code.to_string());
                    out.push(';');
                } else {
                    if add_zero_width_spaces {
                        out.push_str("&#8203;");
                    }
                    out.push(other);
                }
            }
        }
    }
    out
}

/// True when `URLDecoder` would leave the text unchanged: letters, digits,
/// `._-*+`, and well-formed `%HH` escapes.
fn is_url_decoder_safe(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch.is_alphanumeric() || matches!(ch, '.' | '_' | '-' | '*' | '+') {
            i += 1;
            continue;
        }
        if ch == '%' && i + 2 < chars.len() && is_hex(chars[i + 1]) && is_hex(chars[i + 2]) {
            i += 3;
            continue;
        }
        return false;
    }
    true
}

/// `URLEncoder.encode` when the text is not already decoder-safe. Space is `+`.
pub fn url_encode_if_needed(text: &str) -> String {
    if is_url_decoder_safe(text) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => out.push(*byte as char),
            b' ' => out.push('+'),
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push('%');
                out.push(HEX[(byte >> 4) as usize] as char);
                out.push(HEX[(byte & 0x0F) as usize] as char);
            }
        }
    }
    out
}

fn is_hex(ch: char) -> bool {
    ch.is_ascii_hexdigit()
}

/// `camelCaseToPretty`. A part equal to `id` in any case becomes `ID`.
pub fn camel_case_to_pretty(camel: &str) -> String {
    if camel.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = camel.chars().collect();
    let mut raw_parts = Vec::new();
    let mut current = String::new();
    for (index, ch) in chars.iter().copied().enumerate() {
        if index > 0
            && (ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '.' || ch == '#')
            && !current.is_empty()
        {
            raw_parts.push(std::mem::take(&mut current));
        }
        current.push(ch);
    }
    if !current.is_empty() {
        raw_parts.push(current);
    }
    let mut pretty = String::new();
    let mut last: Option<String> = None;
    for part in raw_parts {
        let mut part = match part.chars().next() {
            Some('.' | '#') => part.chars().skip(1).collect(),
            _ => part,
        };
        if part.is_empty() {
            continue;
        }
        let mut part_chars: Vec<char> = part.chars().collect();
        if part_chars[0].is_lowercase() {
            part_chars[0] = part_chars[0].to_ascii_uppercase();
        }
        part = part_chars.into_iter().collect();
        if part.eq_ignore_ascii_case("id") {
            part = "ID".to_string();
        }
        if last.as_ref().is_some_and(|prev| prev == &part) {
            continue;
        }
        last = Some(part.clone());
        if !pretty.is_empty() {
            pretty.push(' ');
        }
        pretty.push_str(&part);
    }
    pretty
}

/// `prettyToCamelCase`. Characters that are not letters or digits start a new word.
pub fn pretty_to_camel_case(pretty: &str, first_upper: bool) -> String {
    let mut out = String::new();
    let mut upper_next = first_upper;
    for ch in pretty.chars() {
        if ch.is_alphanumeric() {
            if upper_next {
                out.extend(ch.to_uppercase());
            } else {
                out.extend(ch.to_lowercase());
            }
            upper_next = false;
        } else {
            upper_next = true;
        }
    }
    out
}

/// One position of a `MaskFormatter` mask.
#[derive(Clone, Debug)]
enum MaskChar {
    Literal(char),
    /// `#`
    Digit,
    /// `U`, upper-cased
    Upper,
    /// `L`, lower-cased
    Lower,
    /// `A`
    LetterOrDigit,
    /// `?`
    Letter,
    /// `*`
    Any,
    /// `H`, upper-cased
    Hex,
}

/// `StringUtilities.masker`: a `javax.swing.text.MaskFormatter` whose value has no literal characters.
#[derive(Clone, Debug)]
pub struct Masker {
    mask: Vec<MaskChar>,
    /// `setPlaceholder`, for a placeholder longer than one character.
    placeholder: Option<Vec<char>>,
    /// `setPlaceholderCharacter`; a space unless the placeholder is one character.
    placeholder_char: char,
}

/// `StringUtilities.masker(mask, placeholder)`: no formatter for an empty mask.
pub fn masker(mask: &str, placeholder: &str) -> Option<Masker> {
    if mask.is_empty() {
        return None;
    }
    let mut chars = mask.chars();
    let mut parsed = Vec::new();
    while let Some(ch) = chars.next() {
        parsed.push(match ch {
            '#' => MaskChar::Digit,
            'U' => MaskChar::Upper,
            'L' => MaskChar::Lower,
            'A' => MaskChar::LetterOrDigit,
            '?' => MaskChar::Letter,
            '*' => MaskChar::Any,
            'H' => MaskChar::Hex,
            // A quote makes the next character a literal; a quote at the end of the mask is itself one.
            '\'' => MaskChar::Literal(chars.next().unwrap_or('\'')),
            other => MaskChar::Literal(other),
        });
    }
    let placeholder_chars: Vec<char> = placeholder.chars().collect();
    let (placeholder, placeholder_char) = match placeholder_chars.len() {
        0 => (None, ' '),
        1 => (None, placeholder_chars[0]),
        _ => (Some(placeholder_chars), ' '),
    };
    Some(Masker {
        mask: parsed,
        placeholder,
        placeholder_char,
    })
}

impl Masker {
    /// `MaskFormatter.valueToString`: each mask character takes the next value character, a literal is inserted, and
    /// past the end of the value the placeholder fills in. A character the mask does not accept is an error, as
    /// Java's `ParseException`; value characters beyond the mask are dropped.
    pub fn value_to_string(&self, value: &str) -> Result<String, String> {
        let value: Vec<char> = value.chars().collect();
        let mut out = String::new();
        let mut index = 0;
        for mask_char in &self.mask {
            if let MaskChar::Literal(literal) = mask_char {
                out.push(*literal);
                continue;
            }
            let Some(&ch) = value.get(index) else {
                match self.placeholder.as_ref().and_then(|placeholder| placeholder.get(index)) {
                    Some(&placeholder) => out.push(placeholder),
                    None => out.push(self.placeholder_char),
                }
                index += 1;
                continue;
            };
            let valid = match mask_char {
                MaskChar::Digit => ch.is_numeric(),
                MaskChar::Upper | MaskChar::Lower | MaskChar::Letter => ch.is_alphabetic(),
                MaskChar::LetterOrDigit => ch.is_alphanumeric(),
                MaskChar::Any => true,
                MaskChar::Hex => ch.is_ascii_hexdigit(),
                MaskChar::Literal(_) => unreachable!(),
            };
            if !valid {
                return Err(format!("Invalid character: {ch}"));
            }
            match mask_char {
                MaskChar::Upper | MaskChar::Hex => out.extend(ch.to_uppercase()),
                MaskChar::Lower => out.extend(ch.to_lowercase()),
                _ => out.push(ch),
            }
            index += 1;
        }
        Ok(out)
    }
}

/// Drop a leading UTF-8 BOM and decode. Invalid sequences become U+FFFD,
/// which is what `new String(bytes, "UTF-8")` does.
pub fn to_string_clean_bom(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masker_formats_as_mask_formatter() {
        assert!(masker("", "0").is_none());
        let gl = masker("###-###-###", "0").unwrap();
        assert_eq!(gl.value_to_string("149300000").unwrap(), "149-300-000");
        // A short value is filled with the placeholder; extra characters are dropped.
        assert_eq!(gl.value_to_string("2120").unwrap(), "212-000-000");
        assert_eq!(gl.value_to_string("1234567890").unwrap(), "123-456-789");
        assert_eq!(gl.value_to_string("").unwrap(), "000-000-000");
        assert!(gl.value_to_string("12a").is_err());
        assert_eq!(masker("UL-'#H", "").unwrap().value_to_string("abf").unwrap(), "Ab-#F");
        assert_eq!(masker("##-##", "abcd").unwrap().value_to_string("1").unwrap(), "1b-cd");
    }

    #[test]
    fn xml_attribute_encoding_and_decoding() {
        assert_eq!(
            encode_for_xml_attribute(r#"a&b<c>"d'e"#, false),
            "a&amp;b&lt;c&gt;&quot;d&apos;e"
        );
        assert_eq!(encode_for_xml_attribute("\u{0005}\u{0001}A", false), "...A");
        assert_eq!(encode_for_xml_attribute("é", false), "&#233;");
    }

    #[test]
    fn urls_paths_and_pretty_names() {
        assert_eq!(url_encode_if_needed("a b"), "a+b");
        assert_eq!(url_encode_if_needed("a-b_c.d*e"), "a-b_c.d*e");
        assert_eq!(url_encode_if_needed("%20"), "%20");
        assert_eq!(url_encode_if_needed("%2"), "%252");
        assert_eq!(camel_case_to_pretty("orderId"), "Order ID");
        assert_eq!(camel_case_to_pretty("foo.bar"), "Foo Bar");
        // The browser copy in WebrootVue.qvt.js (qvt.StringUtilities) is checked against the same vectors.
        assert_eq!(camel_case_to_pretty("productStoreId"), "Product Store ID");
        assert_eq!(camel_case_to_pretty("toFieldNameAlias"), "To Field Name Alias");
        assert_eq!(camel_case_to_pretty("aaAa"), "Aa");
        assert_eq!(camel_case_to_pretty("item2Id"), "Item 2 ID");
        assert_eq!(camel_case_to_pretty("#hash"), "Hash");
        assert_eq!(camel_case_to_pretty(""), "");
        assert_eq!(pretty_to_camel_case("Order ID", false), "orderId");
        assert_eq!(pretty_to_camel_case("order id", true), "OrderId");
    }

    #[test]
    fn clean_bom() {
        let bom = b"\xEF\xBB\xBFhi";
        assert_eq!(to_string_clean_bom(bom), "hi");
    }
}
