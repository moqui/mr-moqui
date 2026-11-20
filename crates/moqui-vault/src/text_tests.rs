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

#[test]
fn base32_round_trips_and_forgives_look_alikes() {
    let bytes: Vec<u8> = (0..=40).collect();
    let text = base32_grouped(&bytes, 4);
    assert!(text.split('-').all(|group| group.len() <= 4));
    assert_eq!(&base32_decode(&text).unwrap()[..], &bytes[..]);
    let typed = text
        .to_lowercase()
        .replace('-', " ")
        .replace('0', "o")
        .replace('1', "l");
    assert_eq!(&base32_decode(&typed).unwrap()[..], &bytes[..]);
    assert!(base32_decode("ABCU").is_err());
}

#[test]
fn hex_round_trips() {
    assert_eq!(hex(&[0, 15, 16, 255]), "000f10ff");
    assert_eq!(unhex("000f10FF").unwrap(), vec![0, 15, 16, 255]);
    assert!(unhex("abc").is_err());
    assert!(unhex("zz").is_err());
}

#[test]
fn the_arithmetic_maps_agree_with_the_alphabet_for_every_character() {
    // The reference: the Crockford alphabet as a table, used only here.
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    for v in 0..32u16 {
        assert_eq!(base32_char(v), char::from(ALPHABET[usize::from(v)]), "value {v}");
    }
    for c in 0..=255u8 {
        let upper = c.to_ascii_uppercase();
        let expected = match upper {
            b'O' => Some(0),
            b'I' | b'L' => Some(1),
            _ => ALPHABET.iter().position(|&a| a == upper).map(|at| at as u16),
        };
        let (value, valid) = base32_value(c);
        match expected {
            Some(v) => assert_eq!((value, valid), (v, 1), "char {c}"),
            None => assert_eq!(valid, 0, "char {c}"),
        }
    }
    for c in 0..=255u8 {
        let text = [c, c];
        let expected = char::from(c).to_digit(16);
        let text = std::str::from_utf8(&text).ok();
        if let Some(text) = text {
            assert_eq!(unhex(text).ok(), expected.map(|d| vec![(d * 17) as u8]), "char {c}");
        }
    }
}

#[test]
fn a_bad_character_anywhere_is_refused() {
    assert!(base32_decode("U0000000").is_err());
    assert!(base32_decode("0000000U").is_err());
    assert!(unhex("g0").is_err());
    assert!(unhex("0g").is_err());
}
