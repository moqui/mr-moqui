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
use crate::rng::TestRng;

/// Every subset of `0..n` with `k` members.
fn subsets(n: usize, k: usize) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    for mask in 0u32..(1 << n) {
        if mask.count_ones() as usize == k {
            out.push((0..n).filter(|bit| mask & (1 << bit) != 0).collect());
        }
    }
    out
}

fn pick(shares: &[Share], at: &[usize]) -> Vec<Share> {
    at.iter().map(|&i| shares[i].clone()).collect()
}

#[test]
fn field_multiplication_and_inverse() {
    // FIPS 197 §4.2: {57} • {83} = {c1}, {57} • {13} = {fe}.
    assert_eq!(gf_mul(0x57, 0x83), 0xc1);
    assert_eq!(gf_mul(0x57, 0x13), 0xfe);
    for a in 0..=255u8 {
        assert_eq!(gf_mul(a, 1), a);
        assert_eq!(gf_mul(a, 0), 0);
        if a != 0 {
            assert_eq!(gf_mul(a, gf_inv(a)), 1, "inverse of {a}");
        }
    }
    assert_eq!(gf_inv(0), 0);
}

#[test]
fn every_k_subset_recovers_the_secret() {
    let mut rng = TestRng::new("shamir-subsets");
    let secret: Vec<u8> = (0..32).map(|i| i * 7 + 3).collect();
    for (k, n) in [(2u8, 2u8), (2, 3), (3, 5), (4, 6)] {
        let shares = split(&secret, k, n, &mut rng).unwrap();
        assert_eq!(shares.len(), usize::from(n));
        for size in usize::from(k)..=usize::from(n) {
            for at in subsets(usize::from(n), size) {
                let got = combine(&pick(&shares, &at)).unwrap();
                assert_eq!(&got[..], &secret[..], "k={k} n={n} shares {at:?}");
            }
        }
    }
}

#[test]
fn many_shares_recover_in_any_order() {
    let mut rng = TestRng::new("shamir-255");
    let shares = split(b"a longer secret value", 3, 255, &mut rng).unwrap();
    assert_eq!(shares.last().unwrap().index, 255);
    let reversed: Vec<Share> = [254usize, 100, 0].iter().map(|&i| shares[i].clone()).collect();
    assert_eq!(&combine(&reversed).unwrap()[..], b"a longer secret value");
}

#[test]
fn fewer_than_k_shares_do_not_give_the_secret() {
    let mut rng = TestRng::new("shamir-fewer");
    let secret = [0x42u8; 32];
    let shares = split(&secret, 3, 5, &mut rng).unwrap();
    for at in subsets(5, 2) {
        assert_ne!(&combine(&pick(&shares, &at)).unwrap()[..], &secret[..], "{at:?}");
    }
}

#[test]
fn k_minus_one_shares_look_the_same_for_any_secret() {
    // With k=3 two shares are uniform whatever the secret: the byte histogram of shares 1 and 2 over many splits is
    // flat for an all-zero secret and for an all-0xff one alike (a chi-square test, 255 degrees of freedom; 400 is
    // far in the tail, so a fixed stream passes it every time unless the shares carry the secret).
    for secret_byte in [0x00u8, 0xff] {
        let mut rng = TestRng::new("shamir-uniform");
        let secret = [secret_byte; 64];
        let mut counts = [0u32; 256];
        let rounds = 200;
        for _ in 0..rounds {
            let shares = split(&secret, 3, 5, &mut rng).unwrap();
            for share in &shares[..2] {
                for &byte in share.value.iter() {
                    counts[usize::from(byte)] += 1;
                }
            }
        }
        let expected = f64::from(rounds * 2 * 64) / 256.0;
        let chi_square: f64 = counts
            .iter()
            .map(|&c| (f64::from(c) - expected).powi(2) / expected)
            .sum();
        assert!(
            chi_square < 400.0,
            "secret byte {secret_byte:#x}: chi-square {chi_square}"
        );
    }
}

#[test]
fn split_refuses_k_and_n_out_of_range() {
    let mut rng = TestRng::new("shamir-range");
    assert!(matches!(split(b"s", 1, 5, &mut rng), Err(Error::Shares(_))));
    assert!(matches!(split(b"s", 0, 5, &mut rng), Err(Error::Shares(_))));
    assert!(matches!(split(b"s", 4, 3, &mut rng), Err(Error::Shares(_))));
    assert!(matches!(split(b"", 2, 3, &mut rng), Err(Error::Shares(_))));
}

#[test]
fn combine_refuses_duplicates_and_mismatched_lengths() {
    let mut rng = TestRng::new("shamir-refuse");
    let shares = split(b"secret", 2, 3, &mut rng).unwrap();
    assert!(matches!(combine(&[]), Err(Error::Shares(_))));
    assert!(matches!(combine(&pick(&shares, &[0])), Err(Error::Shares(_))));
    assert!(matches!(combine(&pick(&shares, &[1, 1])), Err(Error::Shares(_))));
    let mut short = shares[1].clone();
    short.value.pop();
    assert!(matches!(combine(&[shares[0].clone(), short]), Err(Error::Shares(_))));
    let zero = Share {
        index: 0,
        value: shares[1].value.clone(),
    };
    assert!(matches!(combine(&[shares[0].clone(), zero]), Err(Error::Shares(_))));
}

#[test]
fn text_round_trips_and_forgives_case_and_look_alikes() {
    let mut rng = TestRng::new("shamir-text");
    let shares = split(&[9u8; 32], 3, 5, &mut rng).unwrap();
    for share in &shares {
        let text = share.to_text();
        assert!(text.starts_with(&format!("share-{:02x}-", share.index)), "{}", &*text);
        let back = Share::from_text(&text).unwrap();
        assert_eq!(back.index, share.index);
        assert_eq!(back.value, share.value);
        // Lower case, spaces for dashes and O for 0 still read.
        let typed = text.to_lowercase().replace('-', " ").replace('0', "o");
        assert_eq!(Share::from_text(&typed).unwrap().value, share.value);
    }
}

#[test]
fn a_typo_in_a_share_is_caught() {
    let mut rng = TestRng::new("shamir-typo");
    let share = split(&[5u8; 32], 2, 2, &mut rng).unwrap().remove(0);
    let text = share.to_text();
    let mut chars: Vec<char> = text.chars().collect();
    let at = (chars.len() / 2..).find(|&i| chars[i] != '-').unwrap();
    chars[at] = if chars[at] == 'A' { 'B' } else { 'A' };
    let typo: String = chars.into_iter().collect();
    assert!(matches!(Share::from_text(&typo), Err(Error::Format(_))));
    // A changed index, outside the checksummed part, is caught too.
    let moved = text.replacen("share-01", "share-02", 1);
    assert!(matches!(Share::from_text(&moved), Err(Error::Format(_))));
    assert!(matches!(Share::from_text("not a share"), Err(Error::Format(_))));
    assert!(matches!(Share::from_text("share-01-UUUUU"), Err(Error::Format(_))));
}

#[test]
fn debug_never_shows_a_share_value() {
    let share = Share {
        index: 3,
        value: Zeroizing::new(vec![0xab; 4]),
    };
    assert_eq!(format!("{share:?}"), "Share { index: 3, value: redacted (4 bytes) }");
}
