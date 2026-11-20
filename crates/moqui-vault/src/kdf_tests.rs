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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn parameters_under_the_floor_are_refused() {
    let salt = [0u8; 16];
    let floor = Argon2Params::FLOOR;
    for under in [
        Argon2Params {
            m_kib: floor.m_kib - 1,
            ..floor
        },
        Argon2Params {
            t: floor.t - 1,
            ..floor
        },
        Argon2Params { p: 0, ..floor },
        Argon2Params { m_kib: 8, t: 1, p: 1 },
    ] {
        assert!(
            matches!(argon2id(b"pw", &salt, under), Err(Error::Params(_))),
            "{under:?}"
        );
    }
}

#[test]
fn parameters_over_the_ceiling_are_refused() {
    let salt = [0u8; 16];
    let ceiling = Argon2Params::CEILING;
    for over in [
        Argon2Params {
            m_kib: ceiling.m_kib + 1,
            ..Argon2Params::FLOOR
        },
        Argon2Params {
            t: ceiling.t + 1,
            ..Argon2Params::FLOOR
        },
        Argon2Params {
            p: ceiling.p + 1,
            ..Argon2Params::FLOOR
        },
        Argon2Params {
            m_kib: u32::MAX,
            t: u32::MAX,
            p: u32::MAX,
        },
    ] {
        assert!(
            matches!(argon2id(b"pw", &salt, over), Err(Error::Params(_))),
            "{over:?}"
        );
    }
}

#[test]
fn a_short_salt_is_refused() {
    assert!(matches!(
        argon2id(b"pw", &[0u8; 15], Argon2Params::FLOOR),
        Err(Error::Params(_))
    ));
}

#[test]
fn under_the_target_signals_a_rewrap() {
    assert!(!needs_rewrap(Argon2Params::TARGET));
    assert!(!needs_rewrap(Argon2Params {
        m_kib: Argon2Params::TARGET.m_kib * 2,
        ..Argon2Params::TARGET
    }));
    assert!(needs_rewrap(Argon2Params {
        m_kib: Argon2Params::TARGET.m_kib - 1,
        ..Argon2Params::TARGET
    }));
    assert!(needs_rewrap(Argon2Params {
        t: Argon2Params::TARGET.t - 1,
        ..Argon2Params::TARGET
    }));
    // The target is never under the floor, so what is written always opens.
    assert!(Argon2Params::TARGET.check().is_ok());
    assert!(Argon2Params::CEILING.check().is_ok());
}

#[test]
fn argon2id_at_the_floor_depends_on_the_salt() {
    let a = argon2id(b"correct horse", b"0123456789abcdef", Argon2Params::FLOOR).unwrap();
    let b = argon2id(b"correct horse", b"0123456789abcdeF", Argon2Params::FLOOR).unwrap();
    assert_ne!(a, b);
}

#[test]
fn hkdf_matches_rfc_5869_case_1() {
    let ikm = [0x0b; 22];
    let salt: Vec<u8> = (0x00..=0x0c).collect();
    let info: Vec<u8> = (0xf0..=0xf9).collect();
    let okm = hkdf_sha256(&ikm, &salt, &info);
    // The first 32 bytes of the RFC's 42-byte OKM.
    assert_eq!(
        hex(okm.as_bytes()),
        "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf"
    );
}
