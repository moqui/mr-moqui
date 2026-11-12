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

use std::io::Write;

use base64::Engine as _;

use super::{decrypt, Encryptor, Identity, Recipient};

/// A key pair and a file that rage (pyrage 1.x) made, with one of its grease stanzas, whose body is two lines.
const IDENTITY: &str = "AGE-SECRET-KEY-1L30V3C6NPLGY2HV2MY5KH2UE0T7VHJDVD5LUTKW4ARAKFD42K6PQ32HY9N";
const RECIPIENT: &str = "age1wq8w30nsnlgp8nfdg3xrlftccpp5ae48rql8e7hxafjcse769arqjcc753";
const RAGE_FILE: &str = "YWdlLWVuY3J5cHRpb24ub3JnL3YxCi0+IFgyNTUxOSBPY2d5akZGVlRVWGtpOGlyS3BMMkxmd1ZvUy9QWjJkajZFWWx4dExGdVI4Cnp5WDRjRXE5REtmQWQ2KzFXV0UvSnJvODdLc2M4YTFVTEFuV3g2dG5ncXcKLT4gNi1ncmVhc2UgQ01TcCBiV3ZDO09HUCBxVXM7fgpYYm5XTlFubDRDWkJpUi96NEFFMnhCMUZKbWZmNSt2N040dkxUZksrQkxuTUlkZW5mSEpncWplTGRQWkFuYXkwCkMzaHlGVVFqS0FhMDJOY0VRZ2swNm4rdVJRaVpjNHNsMGFvbW44MlgwbEpKUWI5S0RtTQotLS0geVIrQzJJUEFXWnYybHUzUWhvME1FVXpkM2JRRWV5NTVvU0xpMWUzQmZsYwoHJV7gfvLwkzgJPMNLYm7uIURDjibyxMTH9Bj/C27UB9AEJaS5GtCSC0M5SGdrcK4=";

fn encrypt(recipient: &Recipient, plain: &[u8], write_size: usize) -> Vec<u8> {
    let mut writer = Encryptor::new(Vec::new(), recipient).unwrap();
    for piece in plain.chunks(write_size.max(1)) {
        writer.write_all(piece).unwrap();
    }
    writer.finish().unwrap()
}

#[test]
fn keys_parse_and_print_as_rage_does() {
    let identity = Identity::parse(IDENTITY).unwrap();
    assert_eq!(identity.secret_text(), IDENTITY);
    assert_eq!(identity.recipient().to_string(), RECIPIENT);
    assert_eq!(Recipient::parse(RECIPIENT).unwrap(), identity.recipient());
    assert!(Recipient::parse(&RECIPIENT.to_ascii_uppercase()).is_err());
    assert!(Recipient::parse(&RECIPIENT.replace('w', "q")).is_err());
    assert!(Recipient::parse(IDENTITY).is_err());
    assert!(Identity::parse(RECIPIENT).is_err());
}

#[test]
fn a_rage_file_decrypts() {
    let identity = Identity::parse(IDENTITY).unwrap();
    let file = base64::engine::general_purpose::STANDARD.decode(RAGE_FILE).unwrap();
    assert_eq!(decrypt(&identity, &mut file.as_slice()).unwrap(), b"hello from rage\n");
    let mut changed = file.clone();
    let last = changed.len() - 1;
    changed[last] ^= 1;
    assert!(decrypt(&identity, &mut changed.as_slice()).is_err());
    let other = Identity::generate().unwrap();
    assert!(decrypt(&other, &mut file.as_slice()).is_err());
}

#[test]
fn chunks_round_trip_at_every_boundary() {
    let identity = Identity::generate().unwrap();
    let recipient = identity.recipient();
    let chunk = 64 * 1024;
    for len in [0, 1, chunk - 1, chunk, chunk + 1, 2 * chunk, 2 * chunk + 7] {
        let plain: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        for write_size in [1000, chunk, 3 * chunk] {
            let sealed = encrypt(&recipient, &plain, write_size);
            assert_eq!(
                decrypt(&identity, &mut sealed.as_slice()).unwrap(),
                plain,
                "{len}/{write_size}"
            );
        }
        // a full last chunk is the last one, not followed by an empty one
        let sealed = encrypt(&recipient, &plain, chunk);
        let header_end = sealed.windows(4).position(|w| w == b"--- ").unwrap();
        let payload =
            sealed.len() - sealed[header_end..].iter().position(|b| *b == b'\n').unwrap() - header_end - 1 - 16;
        assert_eq!(payload, len + len.div_ceil(chunk).max(1) * 16, "{len}");
        // a file cut at a chunk boundary does not decrypt
        if len > chunk {
            let cut = &sealed[..sealed.len() - payload + chunk + 16];
            assert!(decrypt(&identity, &mut &cut[..]).is_err());
        }
    }
}

/// A stanza whose share is a low-order point (all zeros) gives the all-zero shared secret, a wrap key anyone can
/// compute: the file is refused, as the age spec requires, and encrypting to such a recipient is refused too.
#[test]
fn a_low_order_share_or_recipient_is_refused() {
    let identity = Identity::generate().unwrap();
    let zero = base64::engine::general_purpose::STANDARD_NO_PAD.encode([0_u8; 32]);
    let file = format!("age-encryption.org/v1\n-> X25519 {zero}\n{zero}\n--- {zero}\n");
    let err = decrypt(&identity, &mut file.as_bytes()).unwrap_err();
    assert!(err.to_string().contains("low-order"), "{err}");

    let low_order = Recipient::parse(&Recipient([0_u8; 32]).to_string()).unwrap();
    assert!(Encryptor::new(Vec::new(), &low_order).is_err());
}
