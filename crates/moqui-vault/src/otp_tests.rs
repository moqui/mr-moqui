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

const SHA1_KEY: &[u8] = b"12345678901234567890";
const SHA256_KEY: &[u8] = b"12345678901234567890123456789012";
const SHA512_KEY: &[u8] = b"1234567890123456789012345678901234567890123456789012345678901234";

fn seed(key: &[u8], algorithm: Algorithm, digits: u32) -> Seed {
    Seed {
        key: Zeroizing::new(key.to_vec()),
        algorithm,
        digits,
        period: 30,
        issuer: None,
        account: None,
    }
}

/// RFC 4226 Appendix D.
#[test]
fn the_rfc_4226_vectors() {
    let expected = [
        "755224", "287082", "359152", "969429", "338314", "254676", "287922", "162583", "399871", "520489",
    ];
    for (counter, code) in expected.iter().enumerate() {
        assert_eq!(hotp(SHA1_KEY, counter as u64, Algorithm::Sha1, 6), *code, "{counter}");
    }
}

/// RFC 6238 Appendix B, eight digits, for each algorithm.
#[test]
fn the_rfc_6238_vectors() {
    let rows: [(i64, &str, &str, &str); 6] = [
        (59, "94287082", "46119246", "90693936"),
        (1_111_111_109, "07081804", "68084774", "25091201"),
        (1_111_111_111, "14050471", "67062674", "99943326"),
        (1_234_567_890, "89005924", "91819424", "93441116"),
        (2_000_000_000, "69279037", "90698825", "38618901"),
        (20_000_000_000, "65353130", "77737706", "47863826"),
    ];
    for (time, sha1, sha256, sha512) in rows {
        let at = time * 1000;
        assert_eq!(codes(&seed(SHA1_KEY, Algorithm::Sha1, 8), at).code, sha1, "{time}");
        assert_eq!(
            codes(&seed(SHA256_KEY, Algorithm::Sha256, 8), at).code,
            sha256,
            "{time}"
        );
        assert_eq!(
            codes(&seed(SHA512_KEY, Algorithm::Sha512, 8), at).code,
            sha512,
            "{time}"
        );
        assert_eq!(codes(&seed(SHA1_KEY, Algorithm::Sha1, 6), at).code, sha1[2..], "{time}");
    }
}

#[test]
fn the_next_code_and_the_time_left() {
    let seed = seed(SHA1_KEY, Algorithm::Sha1, 8);
    let now = codes(&seed, 59_000);
    assert_eq!(now.remaining_ms, 1000);
    assert_eq!(now.next, hotp(SHA1_KEY, 2, Algorithm::Sha1, 8));
    assert_eq!(codes(&seed, 60_000).remaining_ms, 30_000);
    assert_eq!(codes(&seed, 60_001).remaining_ms, 29_999);
    assert_eq!(
        codes(&seed, -5).code,
        hotp(SHA1_KEY, 0, Algorithm::Sha1, 8),
        "before the epoch"
    );
}

#[test]
fn base32_round_trips_and_refuses_what_is_not_base32() {
    assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
    assert_eq!(from_base32("MZXW6YTBOI======").unwrap(), b"foobar");
    for length in 0..40 {
        let bytes: Vec<u8> = (0..length).map(|at| (at * 37 + 11) as u8).collect();
        assert_eq!(from_base32(&base32(&bytes)).unwrap(), bytes, "{length}");
    }
    assert!(from_base32("MZXW1").is_none());
}

#[test]
fn a_bare_secret_as_sites_print_it() {
    let parsed = parse("  jbsw y3dp-ehpk 3pxp  ").unwrap();
    assert_eq!(&*parsed.key, b"Hello!\xde\xad\xbe\xef");
    assert_eq!(
        (parsed.algorithm, parsed.digits, parsed.period),
        (Algorithm::Sha1, 6, 30)
    );
    assert_eq!((parsed.issuer, parsed.account), (None, None));
    assert!(parse("not base32!").is_err());
    assert!(parse("").is_err(), "no key");
}

#[test]
fn an_otpauth_uri() {
    let parsed = parse(
        "otpauth://totp/ACME%20Co:jane%40example.com?secret=JBSWY3DPEHPK3PXP&issuer=ACME%20Co&algorithm=SHA256&digits=8&period=60",
    )
    .unwrap();
    assert_eq!(parsed.issuer.as_deref(), Some("ACME Co"));
    assert_eq!(parsed.account.as_deref(), Some("jane@example.com"));
    assert_eq!(
        (parsed.algorithm, parsed.digits, parsed.period),
        (Algorithm::Sha256, 8, 60)
    );
    assert_eq!(
        uri(&parsed),
        "otpauth://totp/ACME%20Co:jane%40example.com?secret=JBSWY3DPEHPK3PXP&issuer=ACME%20Co&algorithm=SHA256&digits=8&period=60"
    );
    assert_eq!(parse(&uri(&parsed)).unwrap(), parsed, "round trip");

    // the issuer from the label when the query has none; the defaults left out of the URI made again
    let label_only = parse("OTPAUTH://totp/GitHub:octo?secret=jbswy3dpehpk3pxp").unwrap();
    assert_eq!(label_only.issuer.as_deref(), Some("GitHub"));
    assert_eq!(label_only.account.as_deref(), Some("octo"));
    assert_eq!(
        uri(&label_only),
        "otpauth://totp/GitHub:octo?secret=JBSWY3DPEHPK3PXP&issuer=GitHub"
    );
    let account_only = parse("otpauth://totp/octo?secret=JBSWY3DPEHPK3PXP").unwrap();
    assert_eq!(uri(&account_only), "otpauth://totp/octo?secret=JBSWY3DPEHPK3PXP");
}

#[test]
fn what_is_refused() {
    for (text, why) in [
        ("otpauth://hotp/x?secret=JBSWY3DPEHPK3PXP&counter=1", "counter"),
        ("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&encoder=steam", "Steam"),
        ("otpauth://totp/x?issuer=y", "no secret"),
        ("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&digits=5", "digits"),
        ("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&digits=9", "digits"),
        ("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=0", "period"),
        ("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=7200", "period"),
        ("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&algorithm=MD5", "algorithm"),
        ("otpauth://totp/x%ZZ?secret=JBSWY3DPEHPK3PXP", "bad escape"),
        ("otpauth-migration://offline?data=AA", "a migration"),
        ("otpauth://other/x", "another kind"),
    ] {
        assert!(parse(text).is_err(), "{why}: {text}");
    }
}

#[test]
fn debug_leaves_out_the_key() {
    let shown = format!("{:?}", parse("JBSWY3DPEHPK3PXP").unwrap());
    assert!(shown.contains("redacted") && !shown.contains("72"), "{shown}");
}

/// A protobuf field of bytes.
fn field_bytes(out: &mut Vec<u8>, number: u8, bytes: &[u8]) {
    out.push(number << 3 | 2);
    out.push(bytes.len() as u8);
    out.extend_from_slice(bytes);
}

fn field_varint(out: &mut Vec<u8>, number: u8, value: u8) {
    out.push(number << 3);
    out.push(value);
}

fn base64_of(bytes: &[u8]) -> String {
    const LETTERS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let triple = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for at in 0..=chunk.len() {
            out.push(char::from(LETTERS[((triple >> (18 - 6 * at)) & 63) as usize]));
        }
    }
    while !out.len().is_multiple_of(4) {
        out.push('=');
    }
    out
}

/// An export as Google Authenticator makes it: two time-based seeds and a counter one, which is left out.
#[test]
fn a_google_authenticator_export() {
    let mut first = Vec::new();
    field_bytes(&mut first, 1, b"Hello!\xde\xad\xbe\xef");
    field_bytes(&mut first, 2, b"ACME:jane@example.com");
    field_bytes(&mut first, 3, b"ACME");
    field_varint(&mut first, 4, 1);
    field_varint(&mut first, 5, 1);
    field_varint(&mut first, 6, 2);
    let mut second = Vec::new();
    field_bytes(&mut second, 1, SHA256_KEY);
    field_bytes(&mut second, 2, b"Gitea:octo");
    field_varint(&mut second, 4, 2);
    field_varint(&mut second, 5, 2);
    field_varint(&mut second, 6, 2);
    let mut counter = Vec::new();
    field_bytes(&mut counter, 1, SHA1_KEY);
    field_bytes(&mut counter, 2, b"old");
    field_varint(&mut counter, 6, 1);
    field_varint(&mut counter, 7, 5);
    let mut payload = Vec::new();
    field_bytes(&mut payload, 1, &first);
    field_bytes(&mut payload, 1, &second);
    field_bytes(&mut payload, 1, &counter);
    field_varint(&mut payload, 2, 1);
    field_varint(&mut payload, 3, 1);
    let data = base64_of(&payload)
        .replace('+', "%2B")
        .replace('/', "%2F")
        .replace('=', "%3D");

    let seeds = migration(&format!("otpauth-migration://offline?data={data}")).unwrap();
    assert_eq!(seeds.len(), 2, "the counter seed is left out");
    assert_eq!(&*seeds[0].key, b"Hello!\xde\xad\xbe\xef");
    assert_eq!(seeds[0].issuer.as_deref(), Some("ACME"));
    assert_eq!(seeds[0].account.as_deref(), Some("jane@example.com"));
    assert_eq!((seeds[0].algorithm, seeds[0].digits), (Algorithm::Sha1, 6));
    assert_eq!(seeds[1].issuer.as_deref(), Some("Gitea"), "the issuer from the name");
    assert_eq!(seeds[1].account.as_deref(), Some("octo"));
    assert_eq!((seeds[1].algorithm, seeds[1].digits), (Algorithm::Sha256, 8));

    assert!(migration("otpauth-migration://offline").is_err(), "no data");
    assert!(migration("otpauth-migration://offline?data=%%%").is_err(), "not base64");
    let truncated = base64_of(&payload[..payload.len() / 2]);
    assert!(
        migration(&format!("otpauth-migration://offline?data={truncated}")).is_err(),
        "truncated"
    );
}
