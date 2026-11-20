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
