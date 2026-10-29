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

use super::*;

/// The Code 128 text of `widths`, read back with the same table, the checksum checked.
fn decode(widths: &[u8]) -> String {
    assert!(widths.ends_with(&[2, 3, 3, 1, 1, 1, 2]), "no stop: {widths:?}");
    let symbols = &widths[..widths.len() - 7];
    assert_eq!(symbols.len() % 6, 0);
    let values: Vec<u8> = symbols
        .chunks(6)
        .map(|chunk| {
            let pattern: Vec<u8> = chunk.iter().map(|width| width + b'0').collect();
            let found = CODE128.iter().position(|entry| entry.as_slice() == pattern.as_slice());
            u8::try_from(found.expect("a Code 128 symbol")).unwrap()
        })
        .collect();
    let (check, data) = values.split_last().unwrap();
    let sum: usize = data
        .iter()
        .enumerate()
        .map(|(at, value)| at.max(1) * usize::from(*value))
        .sum();
    assert_eq!(sum % 103, usize::from(*check), "checksum");
    let mut set = match data[0] {
        103 => Set::A,
        104 => Set::B,
        105 => Set::C,
        other => panic!("start {other}"),
    };
    let mut text = String::new();
    for value in &data[1..] {
        match (set, *value) {
            (_, TO_C) if set != Set::C => set = Set::C,
            (Set::A | Set::C, TO_B) => set = Set::B,
            (Set::B | Set::C, TO_A) => set = Set::A,
            (Set::C, pair) => text.push_str(&format!("{pair:02}")),
            (Set::A, value) if value >= 64 => text.push(char::from(value - 64)),
            (_, value) => text.push(char::from(value + 32)),
        }
    }
    text
}

fn digits(widths: &[u8]) -> String {
    widths.iter().map(|width| char::from(width + b'0')).collect()
}

#[test]
fn each_symbol_is_eleven_modules_with_even_bars() {
    for (value, pattern) in CODE128.iter().enumerate() {
        let widths: Vec<u32> = pattern.iter().map(|digit| u32::from(digit - b'0')).collect();
        assert_eq!(widths.iter().sum::<u32>(), 11, "{value}");
        assert_eq!((widths[0] + widths[2] + widths[4]) % 2, 0, "{value}");
    }
    let unique: std::collections::BTreeSet<_> = CODE128.iter().collect();
    assert_eq!(unique.len(), CODE128.len());
}

/// Bars as barcode4j 2.1 (the library of Java's FOP extension) draws them: the same code sets, symbols, check
/// symbol and stop.
#[test]
fn the_bars_are_barcode4j_bars() {
    for (text, expected) in [
        ("ABC123", "2112141113231311231313211232212232112211321411222331112"),
        ("123456", "2112321122321311233311211321312331112"),
        ("55500", "2112142132121131413113212122222211142331112"),
        (
            "DEMO_1_1",
            "2112141123131321131131231331211112241232211112241232212121412331112",
        ),
        (
            "Hello, World!",
            "2112142311131122142211142211141341111122322122223113211341111212412211141412212221222211142331112",
        ),
        ("100045", "2112322213122122221131231321312331112"),
        (
            "ab12345678cd",
            "2112141211241214211131411122321311233311212411121141311411221412211421122331112",
        ),
        ("1234567", "2112141232211131413121311131231411221341112331112"),
    ] {
        assert_eq!(digits(&code128(text).unwrap().widths), expected, "{text}");
    }
}

#[test]
fn every_ascii_text_reads_back() {
    let printable: String = (32u8..127).map(char::from).collect();
    for text in [
        "12",
        "1",
        "123",
        "1234AB",
        "12345AB",
        "AB1234",
        "AB12345",
        "A1234567B",
        "\tTAB\nline",
        "lower\u{1}control",
        "a\u{1f}b`c",
        "00",
        "0000000",
        printable.as_str(),
    ] {
        assert_eq!(decode(&code128(text).unwrap().widths), text, "{text:?}");
    }
}

#[test]
fn text_outside_ascii_or_empty_is_an_error() {
    assert!(code128("caf\u{e9}").unwrap_err().message().contains("ASCII only"));
    assert!(code128("").is_err());
}

#[test]
fn lengths_take_the_css_units() {
    let near = |text: &str, mm: f64| assert!((length_mm(text).unwrap() - mm).abs() < 1e-9, "{text}");
    near("0.4in", 10.16);
    near("12pt", 12.0 * 25.4 / 72.0);
    near("0.3mm", 0.3);
    near(" 1.5 cm ", 15.0);
    near("96px", 25.4);
    near("5", 5.0);
    assert!(length_mm("4em").is_err());
    assert!(length_mm("wide").is_err());
}

#[test]
fn the_svg_has_the_quiet_zones_bars_and_text_in_millimetres() {
    let options = SvgOptions {
        height: length_mm("0.4in").unwrap(),
        module_width: 0.3,
        font_size: length_mm("12pt").unwrap(),
        ..SvgOptions::default()
    };
    let barcode = code128("55500").unwrap();
    let svg = barcode.svg(&options);
    // 79 modules (start, 4 symbols, check, stop) and 10 each side, 0.3mm a module
    assert!(
        svg.starts_with(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"29.7mm\" height=\"10.16mm\" viewBox=\"0 0 29.7 10.16\""
        ),
        "{svg}"
    );
    // the first bar, two modules wide, after the 3mm quiet zone; the bars leave 1.3 font sizes for the text
    assert!(
        svg.contains("<rect x=\"3\" y=\"0\" width=\"0.6\" height=\"4.6567\"/>"),
        "{svg}"
    );
    assert_eq!(svg.matches("<rect ").count(), barcode.widths.len().div_ceil(2));
    assert!(
        svg.contains(
            "<text x=\"14.85\" y=\"8.5937\" font-family=\"Helvetica, Arial, sans-serif\" font-size=\"4.2333\" \
             text-anchor=\"middle\">55500</text>"
        ),
        "{svg}"
    );
    let plain = barcode.svg(&SvgOptions {
        human_readable: false,
        quiet_zone: 0,
        ..options
    });
    assert!(!plain.contains("<text"));
    assert!(
        plain.contains("<rect x=\"0\" y=\"0\" width=\"0.6\" height=\"10.16\"/>"),
        "{plain}"
    );
    let escaped = code128("A<B&\"C\"").unwrap().svg(&SvgOptions::default());
    assert!(escaped.contains(">A&lt;B&amp;&quot;C&quot;</text>"), "{escaped}");
}

#[test]
fn retail_symbols_require_existing_valid_checksums_and_keep_standard_module_counts() {
    for (kind, code, modules) in [
        ("EAN-13", "4006381333931", 95),
        ("UPC-A", "036000291452", 95),
        ("EAN-8", "96385074", 67),
    ] {
        let encoded = retail(kind, code).unwrap();
        assert_eq!(encoded.widths.iter().map(|w| usize::from(*w)).sum::<usize>(), modules);
        assert_eq!(encoded.text, code);
        let mut invalid = code.to_string();
        invalid.pop();
        invalid.push('0');
        assert!(retail(kind, &invalid).is_err());
        assert!(retail(kind, &code[1..]).is_err());
    }
    assert_eq!(
        retail("UPC-A", "036000291452").unwrap().widths,
        retail("EAN-13", "0036000291452").unwrap().widths
    );
    let itf = retail("ITF-14", "10012345000017").unwrap();
    assert!(itf.widths.starts_with(&[1, 1, 1, 1]));
    assert!(itf.widths.ends_with(&[3, 1, 1]));
    assert_eq!(itf.widths.len(), 77);
    let options = SvgOptions {
        module_width: 0.5,
        height: 36.0,
        ..SvgOptions::default()
    };
    let svg = barcode_svg("ITF-14", "10012345000017", &options).unwrap();
    assert!(svg.contains("width=\"67.5\" height=\"2.5\"")); // bearer bars span the symbol, five modules thick.
}

#[test]
fn matrix_modules_preserve_the_data_and_qr_finder_patterns() {
    let text = "INV-1234567";
    let dm = matrix("DataMatrix", text).unwrap();
    assert_eq!(
        datamatrix::DataMatrix::decode(&dm.bits, dm.width).unwrap(),
        text.as_bytes()
    );
    let qr = matrix("QR", text).unwrap();
    assert_eq!(qr.width, 21);
    assert_eq!(qr.height, 21);
    assert!(qr.bits[0]);
    assert!(!qr.bits[qr.width + 1]);
    assert!(qr.bits[3 * qr.width + 3]);
    for kind in ["QR", "DataMatrix"] {
        assert!(matrix(kind, "").is_err());
        assert!(matrix(kind, &"A".repeat(5000)).is_err());
    }
    let svg = qr.svg(&SvgOptions::default());
    assert!(svg.contains("fill=\"white\""));
    assert!(svg.contains("M10 10h1v1h-1z"));
}

#[test]
fn gs1_scanner_and_human_forms_share_one_validated_element_string() {
    let hri = "(01)04006381333931(17)261231(10)LOT-7(21)SN-9";
    let scanner = "]C101040063813339311726123110LOT-7\u{1d}21SN-9";
    let human = parse_gs1(hri).unwrap();
    let scanned = parse_gs1(scanner).unwrap();
    assert_eq!(human, scanned);
    assert_eq!(human.elements[2].value, "LOT-7");
    assert_eq!(human.elements[3].value, "SN-9");
    assert_eq!(human.human_readable, hri);
    assert!(human.element_string.contains('\u{1d}'));
    for invalid in [
        "(01)04006381333930",
        "(17)260230",
        "(17)261300",
        "(10)",
        "(01)123",
        "(99)unknown",
        "(10)A(10)B",
    ] {
        assert!(parse_gs1(invalid).is_err(), "{invalid}");
    }
    assert!(parse_gs1("(17)260200").is_ok()); // day 00 is the last day of the month.
}

#[test]
fn gs1_128_places_fnc1_at_the_start_and_between_variable_fields() {
    let parsed = parse_gs1("(01)04006381333931(10)LOT(21)SERIAL").unwrap();
    let code = code128_internal(&parsed.element_string, true).unwrap();
    let values: Vec<usize> = code.widths[..code.widths.len() - 7]
        .chunks(6)
        .map(|chunk| {
            CODE128
                .iter()
                .position(|p| p.iter().map(|b| b - b'0').eq(chunk.iter().copied()))
                .unwrap()
        })
        .collect();
    assert_eq!(values[1], 102);
    assert_eq!(values[..values.len() - 1].iter().filter(|v| **v == 102).count(), 2);
    let sum: usize = values[..values.len() - 1]
        .iter()
        .enumerate()
        .map(|(i, v)| i.max(1) * v)
        .sum();
    assert_eq!(values.last(), Some(&(sum % 103)));
    assert!(barcode_svg("GS1-128", "(01)04006381333931", &SvgOptions::default())
        .unwrap()
        .contains("(01)04006381333931"));
    assert!(barcode_svg("unknown", "123", &SvgOptions::default()).is_err());
}
