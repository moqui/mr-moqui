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

//! Linear barcodes as SVG, for print documents (`ec.resource.code128Svg`). Java's XSL-FO documents drew them with the
//! barcode4j FOP extension (`<barcode:code128>`); this is the same symbol and layout as inline SVG.
//!
//! An encoder gives a [`Linear`] barcode, its bar and space widths in modules, and [`Linear::svg`] draws any linear
//! barcode with its quiet zones and the text under the bars. Retail encoders, QR/Data Matrix and inventory GS1
//! parsing share the strict [`barcode_svg`] print entry point.

use crate::error::Error;

/// A linear barcode: the widths in modules of its bars and spaces, starting with a bar, and its human-readable text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Linear {
    pub widths: Vec<u8>,
    pub text: String,
}

/// How [`Linear::svg`] draws a barcode. Lengths are millimetres. The defaults are barcode4j's for Code 128.
#[derive(Clone, Debug, PartialEq)]
pub struct SvgOptions {
    /// The whole height, the text under the bars included.
    pub height: f64,
    /// The width of the narrowest bar.
    pub module_width: f64,
    pub font_size: f64,
    pub font_family: String,
    /// The text under the bars.
    pub human_readable: bool,
    /// The blank margin at each side, in modules.
    pub quiet_zone: u32,
}

impl Default for SvgOptions {
    fn default() -> Self {
        Self {
            height: 15.0,
            module_width: 0.21,
            font_size: 8.0 * MM_PER_POINT,
            font_family: "Helvetica, Arial, sans-serif".to_string(),
            human_readable: true,
            quiet_zone: 10,
        }
    }
}

const MM_PER_INCH: f64 = 25.4;
const MM_PER_POINT: f64 = MM_PER_INCH / 72.0;

/// A length such as `0.4in`, `0.3mm`, `12pt`, `1cm` or `96px`, in millimetres; a number with no unit is millimetres.
///
/// # Errors
///
/// Returns an error when the text is not a number with one of those units.
pub fn length_mm(text: &str) -> Result<f64, Error> {
    let text = text.trim();
    let split = text.find(|ch: char| ch.is_ascii_alphabetic()).unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let value: f64 = number
        .trim()
        .parse()
        .map_err(|_| Error::new(format!("barcode length {text} is not a number with a unit")))?;
    let per_unit = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "mm" => 1.0,
        "cm" => 10.0,
        "in" => MM_PER_INCH,
        "pt" => MM_PER_POINT,
        "px" => MM_PER_INCH / 96.0,
        other => {
            return Err(Error::new(format!(
                "barcode length {text} has unit {other}, not mm, cm, in, pt or px"
            )))
        }
    };
    if !value.is_finite() || value < 0.0 {
        return Err(Error::new(format!("barcode length {text} is not a length")));
    }
    Ok(value * per_unit)
}

impl Linear {
    /// The barcode as an `<svg>` element, sized in millimetres: the quiet zones, the bars, and the text centred under
    /// them in the bottom 1.3 font sizes of the height, as barcode4j lays it out.
    #[must_use]
    pub fn svg(&self, options: &SvgOptions) -> String {
        let module = options.module_width;
        let quiet = f64::from(options.quiet_zone) * module;
        let modules: u32 = self.widths.iter().map(|width| u32::from(*width)).sum();
        let width = quiet * 2.0 + f64::from(modules) * module;
        let height = options.height;
        let text_height = if options.human_readable {
            options.font_size * 1.3
        } else {
            0.0
        };
        let bar_height = (height - text_height).max(height / 4.0);
        let mut svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}mm\" height=\"{h}mm\" viewBox=\"0 0 {w} {h}\" \
             shape-rendering=\"crispEdges\"><g fill=\"black\" stroke=\"none\">",
            w = number(width),
            h = number(height),
        );
        let mut x = quiet;
        for (index, modules) in self.widths.iter().enumerate() {
            let bar = f64::from(*modules) * module;
            if index % 2 == 0 {
                svg.push_str(&format!(
                    "<rect x=\"{}\" y=\"0\" width=\"{}\" height=\"{}\"/>",
                    number(x),
                    number(bar),
                    number(bar_height)
                ));
            }
            x += bar;
        }
        if options.human_readable && !self.text.is_empty() {
            svg.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" font-family=\"{}\" font-size=\"{}\" text-anchor=\"middle\">{}</text>",
                number(width / 2.0),
                number(bar_height + options.font_size * 0.93),
                escape(&options.font_family),
                number(options.font_size),
                escape(&self.text)
            ));
        }
        svg.push_str("</g></svg>");
        svg
    }
}

/// A number for an SVG attribute: at most four decimals, no trailing zeros.
fn number(value: f64) -> String {
    let text = format!("{value:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-" {
        "0".to_string()
    } else {
        text.to_string()
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // XML has no control characters but tab, newline and return
            ch if ch < ' ' && !matches!(ch, '\t' | '\n' | '\r') => out.push(' '),
            ch => out.push(ch),
        }
    }
    out
}

/// The bar and space widths of each Code 128 symbol value, 0 to 105 (ISO/IEC 15417 table 1, as barcode4j has it).
const CODE128: [&[u8; 6]; 106] = [
    b"212222", b"222122", b"222221", b"121223", b"121322", b"131222", b"122213", b"122312", b"132212", b"221213",
    b"221312", b"231212", b"112232", b"122132", b"122231", b"113222", b"123122", b"123221", b"223211", b"221132",
    b"221231", b"213212", b"223112", b"312131", b"311222", b"321122", b"321221", b"312212", b"322112", b"322211",
    b"212123", b"212321", b"232121", b"111323", b"131123", b"131321", b"112313", b"132113", b"132311", b"211313",
    b"231113", b"231311", b"112133", b"112331", b"132131", b"113123", b"113321", b"133121", b"313121", b"211331",
    b"231131", b"213113", b"213311", b"213131", b"311123", b"311321", b"331121", b"312113", b"312311", b"332111",
    b"314111", b"221411", b"431111", b"111224", b"111422", b"121124", b"121421", b"141122", b"141221", b"112214",
    b"112412", b"122114", b"122411", b"142112", b"142211", b"241211", b"221114", b"413111", b"241112", b"134111",
    b"111242", b"121142", b"121241", b"114212", b"124112", b"124211", b"411212", b"421112", b"421211", b"212141",
    b"214121", b"412121", b"111143", b"111341", b"131141", b"114113", b"114311", b"411113", b"411311", b"113141",
    b"114131", b"311141", b"411131", b"211412", b"211214", b"211232",
];
const CODE128_STOP: &[u8; 7] = b"2331112";
const START: [u8; 3] = [103, 104, 105];
const TO_C: u8 = 99;
/// Code B in sets A and C; FNC 4 in set B.
const TO_B: u8 = 100;
/// Code A in sets B and C; FNC 4 in set A.
const TO_A: u8 = 101;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Set {
    A,
    B,
    C,
}

/// The Code 128 barcode of `text`, ASCII only. Digits go in set C, two to a symbol: from the start when the text
/// starts with an even run of four or more, else after one digit in set B, as barcode4j does; later for a run of six or
/// more, or four or more that ends the text. Other characters go in set B, or set A for a control character.
///
/// # Errors
///
/// Returns an error when the text is empty or has a character outside ASCII.
pub fn code128(text: &str) -> Result<Linear, Error> {
    code128_internal(text, false)
}

fn code128_internal(text: &str, gs1: bool) -> Result<Linear, Error> {
    if text.is_empty() {
        return Err(Error::new("a Code 128 barcode needs text"));
    }
    if let Some(ch) = text.chars().find(|ch| !ch.is_ascii()) {
        return Err(Error::new(format!(
            "{ch:?} in {text:?} cannot be encoded in Code 128 (ASCII only)"
        )));
    }
    let bytes = text.as_bytes();
    let digits_at = |at: usize| bytes[at..].iter().take_while(|byte| byte.is_ascii_digit()).count();
    let leading = digits_at(0);
    let mut set = if (leading >= 4 && leading % 2 == 0) || (leading == 2 && bytes.len() == 2) {
        Set::C
    } else {
        a_or_b(&bytes[0..])
    };
    let mut values = vec![START[set as usize]];
    if gs1 {
        values.push(102);
    }
    let mut at = 0;
    while at < bytes.len() {
        if gs1 && bytes[at] == 29 {
            values.push(102);
            at += 1;
            continue;
        }
        if set == Set::C {
            if digits_at(at) >= 2 {
                values.push((bytes[at] - b'0') * 10 + (bytes[at + 1] - b'0'));
                at += 2;
                continue;
            }
            set = a_or_b(&bytes[at..]);
            values.push(if set == Set::A { TO_A } else { TO_B });
            continue;
        }
        let run = digits_at(at);
        if run >= 6 || (run >= 4 && at + run == bytes.len()) {
            if run % 2 == 1 {
                values.push(value_in(set, bytes[at]));
                at += 1;
            }
            values.push(TO_C);
            set = Set::C;
            continue;
        }
        let byte = bytes[at];
        if set == Set::A && byte >= 96 {
            values.push(TO_B);
            set = Set::B;
        } else if set == Set::B && byte < 32 {
            values.push(TO_A);
            set = Set::A;
        }
        values.push(value_in(set, byte));
        at += 1;
    }
    let check = values
        .iter()
        .enumerate()
        .map(|(position, value)| position.max(1) * usize::from(*value))
        .sum::<usize>()
        % 103;
    values.push(u8::try_from(check).unwrap_or_default());
    let mut widths = Vec::with_capacity(values.len() * 6 + 7);
    for value in values {
        widths.extend(CODE128[usize::from(value)].iter().map(|digit| digit - b'0'));
    }
    widths.extend(CODE128_STOP.iter().map(|digit| digit - b'0'));
    Ok(Linear {
        widths,
        text: text.to_string(),
    })
}

/// Set A when a control character comes before a lower case one in `rest`, else set B.
fn a_or_b(rest: &[u8]) -> Set {
    match rest.iter().find(|byte| **byte < 32 || **byte >= 96) {
        Some(byte) if *byte < 32 => Set::A,
        _ => Set::B,
    }
}

/// The value of an ASCII character in set A or B.
fn value_in(set: Set, byte: u8) -> u8 {
    match set {
        Set::A if byte < 32 => byte + 64,
        _ => byte - 32,
    }
}

#[cfg(test)]
#[path = "barcode_tests.rs"]
mod tests;

/// Validate a GS1 mod-10 check digit (GTIN-8/12/13/14 or SSCC-18).
/// The rightmost data digit has weight three; the check digit is never silently replaced.
pub fn valid_check_digit(text: &str) -> bool {
    if text.len() < 2 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let (body, check) = text.split_at(text.len() - 1);
    let sum: u32 = body
        .bytes()
        .rev()
        .enumerate()
        .map(|(i, b)| u32::from(b - b'0') * if i % 2 == 0 { 3 } else { 1 })
        .sum();
    (10 - sum % 10) % 10 == u32::from(check.as_bytes()[0] - b'0')
}

const EAN_L: [&str; 10] = [
    "0001101", "0011001", "0010011", "0111101", "0100011", "0110001", "0101111", "0111011", "0110111", "0001011",
];
const EAN_PARITY: [&str; 10] = [
    "LLLLLL", "LLGLGG", "LLGGLG", "LLGGGL", "LGLLGG", "LGGLLG", "LGGGLL", "LGLGLG", "LGLGGL", "LGGLGL",
];

/// EAN-13, UPC-A, EAN-8 or ITF-14, including a valid existing check digit.
/// Code 128 is available separately; unsupported names and malformed codes return an error.
pub fn retail(symbology: &str, text: &str) -> Result<Linear, Error> {
    let length = match symbology {
        "EAN-13" => 13,
        "UPC-A" => 12,
        "EAN-8" => 8,
        "ITF-14" => 14,
        _ => return Err(Error::new(format!("unsupported retail symbology {symbology}"))),
    };
    if text.len() != length || !valid_check_digit(text) {
        return Err(Error::new(format!(
            "{symbology} requires {length} digits including a valid GS1 check digit"
        )));
    }
    if symbology == "ITF-14" {
        const PATTERNS: [[u8; 5]; 10] = [
            [1, 1, 3, 3, 1],
            [3, 1, 1, 1, 3],
            [1, 3, 1, 1, 3],
            [3, 3, 1, 1, 1],
            [1, 1, 3, 1, 3],
            [3, 1, 3, 1, 1],
            [1, 3, 3, 1, 1],
            [1, 1, 1, 3, 3],
            [3, 1, 1, 3, 1],
            [1, 3, 1, 3, 1],
        ];
        let mut widths = vec![1, 1, 1, 1];
        for pair in text.as_bytes().as_chunks::<2>().0 {
            for (&bar, &space) in PATTERNS[usize::from(pair[0] - b'0')]
                .iter()
                .zip(PATTERNS[usize::from(pair[1] - b'0')].iter())
            {
                widths.push(bar);
                widths.push(space);
            }
        }
        widths.extend([3, 1, 1]);
        return Ok(Linear {
            widths,
            text: text.to_string(),
        });
    }
    let normalized = if symbology == "UPC-A" {
        format!("0{text}")
    } else {
        text.to_string()
    };
    let bytes = normalized.as_bytes();
    let mut bits = "101".to_string();
    let (start, count) = if symbology == "EAN-8" { (0, 4) } else { (1, 6) };
    for i in 0..count {
        let digit = usize::from(bytes[start + i] - b'0');
        if count == 6 && EAN_PARITY[usize::from(bytes[0] - b'0')].as_bytes()[i] == b'G' {
            bits.extend(EAN_L[digit].chars().rev().map(|b| if b == '0' { '1' } else { '0' }));
        } else {
            bits.push_str(EAN_L[digit]);
        }
    }
    bits.push_str("01010");
    for byte in &bytes[start + count..] {
        bits.extend(
            EAN_L[usize::from(*byte - b'0')]
                .chars()
                .map(|b| if b == '0' { '1' } else { '0' }),
        );
    }
    bits.push_str("101");
    let mut widths = Vec::new();
    let mut color = b'1';
    let mut width = 0;
    for bit in bits.bytes() {
        if bit == color {
            width += 1;
        } else {
            widths.push(width);
            color = bit;
            width = 1;
        }
    }
    widths.push(width);
    Ok(Linear {
        widths,
        text: text.to_string(),
    })
}

/// A standards encoder's matrix, stored in row-major order; no raster image or browser code is involved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Matrix {
    pub width: usize,
    pub height: usize,
    pub bits: Vec<bool>,
    pub text: String,
}
impl Matrix {
    pub fn svg(&self, options: &SvgOptions) -> String {
        use std::fmt::Write;
        let quiet = options.quiet_zone as usize;
        let width = self.width + 2 * quiet;
        let height = self.height + 2 * quiet;
        let mut path = String::new();
        for y in 0..self.height {
            for x in 0..self.width {
                if self.bits[y * self.width + x] {
                    let _ = write!(path, "M{} {}h1v1h-1z", x + quiet, y + quiet);
                }
            }
        }
        format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}mm\" height=\"{}mm\" viewBox=\"0 0 {width} {height}\" role=\"img\" aria-label=\"{}\" shape-rendering=\"crispEdges\"><rect width=\"100%\" height=\"100%\" fill=\"white\"/><path d=\"{path}\" fill=\"black\"/></svg>", width as f64 * options.module_width, height as f64 * options.module_width, escape(&self.text))
    }
}

/// QR Model 2 (medium error correction) or Data Matrix ECC 200.
pub fn matrix(symbology: &str, text: &str) -> Result<Matrix, Error> {
    if text.is_empty() || text.len() > 4096 {
        return Err(Error::new("matrix barcode needs 1–4096 bytes"));
    }
    match symbology {
        "QR" => {
            let qr = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Medium)
                .map_err(|e| Error::new(format!("QR: {e:?}")))?;
            let size = qr.size();
            let mut bits = Vec::new();
            for y in 0..size {
                for x in 0..size {
                    bits.push(qr.get_module(x, y));
                }
            }
            Ok(Matrix {
                width: size as usize,
                height: size as usize,
                bits,
                text: text.to_string(),
            })
        }
        "DataMatrix" => {
            let dm = datamatrix::DataMatrix::encode_str(text, datamatrix::SymbolList::default())
                .map_err(|e| Error::new(format!("Data Matrix: {e:?}")))?;
            let bitmap = dm.bitmap();
            Ok(Matrix {
                width: bitmap.width(),
                height: bitmap.height(),
                bits: bitmap.bits().to_vec(),
                text: text.to_string(),
            })
        }
        _ => Err(Error::new(format!("unsupported matrix symbology {symbology}"))),
    }
}

/// How an application identifier's value is written.
struct AiShape {
    /// The length of a fixed-length value; `None` for a variable one.
    fixed: Option<usize>,
    max: usize,
    numeric: bool,
}

/// Supported inventory GS1 application identifiers. A variable field must end with GS/FNC1 before the next AI.
/// Unknown identifiers fail explicitly; this parser never guesses a separator inside a lot or serial number.
fn ai_shape(ai: &str) -> Option<AiShape> {
    match ai {
        "00" => Some(AiShape {
            fixed: Some(18),
            max: 18,
            numeric: true,
        }),
        "01" | "02" => Some(AiShape {
            fixed: Some(14),
            max: 14,
            numeric: true,
        }),
        "11" | "13" | "15" | "16" | "17" => Some(AiShape {
            fixed: Some(6),
            max: 6,
            numeric: true,
        }),
        "20" => Some(AiShape {
            fixed: Some(2),
            max: 2,
            numeric: true,
        }),
        "10" | "21" | "22" => Some(AiShape {
            fixed: None,
            max: 20,
            numeric: false,
        }),
        "30" | "37" => Some(AiShape {
            fixed: None,
            max: 8,
            numeric: true,
        }),
        "240" | "241" | "250" | "251" => Some(AiShape {
            fixed: None,
            max: 30,
            numeric: false,
        }),
        "400" | "401" => Some(AiShape {
            fixed: None,
            max: 30,
            numeric: false,
        }),
        "410" | "411" | "412" | "413" | "414" | "415" | "416" | "417" => Some(AiShape {
            fixed: Some(13),
            max: 13,
            numeric: true,
        }),
        // 310n–316n and 320n, with n (the decimal places) 0–5
        _ if matches!(
            ai.as_bytes(),
            [b'3', b'1', b'0'..=b'6', b'0'..=b'5'] | [b'3', b'2', b'0', b'0'..=b'5']
        ) =>
        {
            Some(AiShape {
                fixed: Some(6),
                max: 6,
                numeric: true,
            })
        }
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gs1Element {
    pub application_identifier: String,
    pub value: String,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gs1Data {
    pub elements: Vec<Gs1Element>,
    pub element_string: String,
    pub human_readable: String,
}

/// GS1 inventory element strings, with an optional AIM prefix (`]C1`, `]d2`, `]Q3`), ASCII GS, or parenthesized HRI.
/// The supported AIs are listed in `ai_shape`. AI 00/01/02/410–417 checksums and YYMMDD calendar dates are checked.
pub fn parse_gs1(input: &str) -> Result<Gs1Data, Error> {
    let input = ["]C1", "]d2", "]Q3"]
        .iter()
        .find_map(|p| input.strip_prefix(p))
        .unwrap_or(input)
        .trim_matches(['\r', '\n']);
    if input.is_empty() || input.len() > 4096 || !input.is_ascii() {
        return Err(Error::new("GS1 input needs 1–4096 ASCII bytes"));
    }
    let hri = input.starts_with('(');
    let mut elements: Vec<Gs1Element> = Vec::new();
    // The input not read yet.
    let mut rest = input;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix(char::from(29)) {
            rest = after;
            if rest.is_empty() {
                break;
            }
        }
        let (ai, after) = if hri {
            rest.strip_prefix('(')
                .ok_or_else(|| Error::new("expected a parenthesized GS1 application identifier"))?
                .split_once(')')
                .ok_or_else(|| Error::new("unclosed GS1 application identifier"))?
        } else {
            [4, 3, 2]
                .into_iter()
                .filter_map(|length| rest.split_at_checked(length))
                .find(|(ai, _)| ai_shape(ai).is_some())
                .ok_or_else(|| {
                    let at = input.len() - rest.len();
                    Error::new(format!("unknown GS1 application identifier at byte {at}"))
                })?
        };
        rest = after;
        let AiShape { fixed, max, numeric } =
            ai_shape(ai).ok_or_else(|| Error::new(format!("unsupported GS1 application identifier {ai}")))?;
        let len = if hri {
            rest.find('(').unwrap_or(rest.len())
        } else if let Some(len) = fixed {
            len
        } else {
            rest.find(char::from(29)).unwrap_or(rest.len())
        };
        let (value, after) = rest
            .split_at_checked(len)
            .ok_or_else(|| Error::new(format!("GS1 {ai} is truncated")))?;
        if value.is_empty()
            || value.len() > max
            || fixed.is_some_and(|len| value.len() != len)
            || numeric && !value.bytes().all(|b| b.is_ascii_digit())
            || value
                .bytes()
                .any(|b| !b.is_ascii_alphanumeric() && !b"!\"%&'()*+,-./:;<=>?_".contains(&b))
        {
            return Err(Error::new(format!("invalid GS1 {ai} value")));
        }
        if matches!(
            ai,
            "00" | "01" | "02" | "410" | "411" | "412" | "413" | "414" | "415" | "416" | "417"
        ) && !valid_check_digit(value)
        {
            return Err(Error::new(format!("GS1 {ai} check digit is invalid")));
        }
        if matches!(ai, "11" | "13" | "15" | "16" | "17") {
            let n = |start, end| {
                value
                    .get(start..end)
                    .and_then(|digits| digits.parse::<u32>().ok())
                    .unwrap_or(99)
            };
            let year = 2000 + n(0, 2);
            let month = n(2, 4);
            let day = n(4, 6);
            let max_day = match month {
                1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
                4 | 6 | 9 | 11 => 30,
                2 => {
                    if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                        29
                    } else {
                        28
                    }
                }
                _ => 0,
            };
            // GS1 permits day 00, meaning the last day of the month.
            if max_day == 0 || day > max_day {
                return Err(Error::new(format!("GS1 {ai} date is invalid")));
            }
        }
        if elements.iter().any(|e| e.application_identifier == ai) {
            return Err(Error::new(format!("duplicate GS1 application identifier {ai}")));
        }
        elements.push(Gs1Element {
            application_identifier: ai.to_string(),
            value: value.to_string(),
        });
        rest = after;
    }
    let mut raw = String::new();
    let mut human = String::new();
    for (i, e) in elements.iter().enumerate() {
        raw.push_str(&e.application_identifier);
        raw.push_str(&e.value);
        human.push('(');
        human.push_str(&e.application_identifier);
        human.push(')');
        human.push_str(&e.value);
        if i + 1 < elements.len() && ai_shape(&e.application_identifier).is_some_and(|shape| shape.fixed.is_none()) {
            raw.push(char::from(29));
        }
    }
    Ok(Gs1Data {
        elements,
        element_string: raw,
        human_readable: human,
    })
}

/// One print entry point for the supported symbologies, with strict input validation.
pub fn barcode_svg(symbology: &str, text: &str, options: &SvgOptions) -> Result<String, Error> {
    if text.is_empty() || text.len() > 4096 {
        return Err(Error::new("barcode needs 1–4096 bytes"));
    }
    if options.quiet_zone > 1000 || !options.font_size.is_finite() || options.font_size < 0.0 {
        return Err(Error::new("invalid barcode font size or quiet zone"));
    }
    if !options.module_width.is_finite()
        || options.module_width <= 0.0
        || !options.height.is_finite()
        || options.height <= 0.0
    {
        return Err(Error::new("barcode dimensions must be positive and finite"));
    }
    match symbology {
        "Code128" | "Code 128" => Ok(code128(text)?.svg(options)),
        "GS1-128" => {
            let parsed = parse_gs1(text)?;
            let mut code = code128_internal(&parsed.element_string, true)?;
            code.text = parsed.human_readable;
            Ok(code.svg(options))
        }
        "ITF-14" => {
            let code = retail(symbology, text)?;
            let height = if options.human_readable {
                (options.height - options.font_size * 1.3).max(options.height / 4.0)
            } else {
                options.height
            };
            let thickness = (options.module_width * 5.0).min(height / 4.0);
            let width = code.widths.iter().map(|w| f64::from(*w)).sum::<f64>() * options.module_width;
            let bars=format!("<rect x=\"{}\" y=\"0\" width=\"{}\" height=\"{}\"/><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/>",number(f64::from(options.quiet_zone)*options.module_width),number(width),number(thickness),number(f64::from(options.quiet_zone)*options.module_width),number(height-thickness),number(width),number(thickness));
            Ok(code.svg(options).replace("</g>", &format!("{bars}</g>")))
        }
        "EAN-13" | "UPC-A" | "EAN-8" => Ok(retail(symbology, text)?.svg(options)),
        "QR" | "DataMatrix" => Ok(matrix(symbology, text)?.svg(options)),
        _ => Err(Error::new(format!("unsupported barcode symbology {symbology}"))),
    }
}
