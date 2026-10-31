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
use crate::rtp::{self, RtpHeader};

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn the_rfc_3711_keystream_and_key_derivation_vectors() {
    // B.2: AES-CM with the IV F0F1…FD0000
    let key: [u8; 16] = hex("2B7E151628AED2A6ABF7158809CF4F3C").try_into().unwrap();
    let iv: [u8; 16] = hex("F0F1F2F3F4F5F6F7F8F9FAFBFCFD0000").try_into().unwrap();
    let mut stream = [0u8; 48];
    aes_cm(&key, &iv, &mut stream);
    assert_eq!(
        stream.to_vec(),
        hex("E03EAD0935C95E80E166B16DD92B4EB4D23513162B02D0F72A43A2FE4A5F97AB41E95B3BB0A2E8DD477901E4FCA894C0")
    );
    // B.3: the session keys from a master key and salt
    let master = MasterKey {
        key: hex("E1F97A0D3E018BE0D64FA32C06DE4139").try_into().unwrap(),
        salt: hex("0EC675AD498AFEEBB6960B3AABE6").try_into().unwrap(),
    };
    let keys = session_keys(&master);
    assert_eq!(keys.key.to_vec(), hex("C61E7A93744F39EE10734AFE3FF7A087"));
    assert_eq!(keys.salt.to_vec(), hex("30CBBC08863D8C85D49DB34A9AE1"));
    assert_eq!(keys.auth.to_vec(), hex("CEBE321F6FF7716B6FD4AB49AF256A156D38BAA4"));
}

fn packet(seq: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    rtp::write(
        &RtpHeader {
            marker: false,
            payload_type: 0,
            sequence: seq,
            timestamp: u32::from(seq) * 160,
            ssrc: 0xcafe_f00d,
        },
        payload,
        &mut out,
    );
    out
}

#[test]
fn packets_round_trip_across_a_rollover_and_tampering_and_replays_are_refused() {
    let master = MasterKey::random();
    assert_eq!(
        MasterKey::from_inline(&format!("inline:{}|2^31", master.inline())),
        Some(master.clone())
    );
    let mut sender = Sender::new(&master);
    let mut receiver = Receiver::new(&master);
    for seq in [65_533u16, 65_534, 65_535, 0, 1, 2] {
        let clear = packet(seq, &[seq as u8; 160]);
        let mut wire = clear.clone();
        sender.protect(&mut wire).unwrap();
        assert_ne!(&wire[12..172], &clear[12..], "the payload is encrypted");
        assert_eq!(wire.len(), clear.len() + 10);
        // a flipped bit fails the tag, and does not use up the packet's index
        let mut bad = wire.clone();
        bad[20] ^= 1;
        assert_eq!(receiver.unprotect(&mut bad), Err(SrtpError::Auth));
        let mut got = wire.clone();
        receiver.unprotect(&mut got).unwrap();
        assert_eq!(got, clear, "seq {seq}");
        // the same packet again is a replay
        let mut again = wire.clone();
        assert_eq!(receiver.unprotect(&mut again), Err(SrtpError::Replay));
    }
    // a packet reordered within the window is taken once
    let mut late_sender = Sender::new(&master);
    let mut late_receiver = Receiver::new(&master);
    let mut wires: Vec<Vec<u8>> = (10u16..14)
        .map(|seq| {
            let mut w = packet(seq, b"voice");
            late_sender.protect(&mut w).unwrap();
            w
        })
        .collect();
    let mut third = wires.remove(2);
    for mut w in wires {
        late_receiver.unprotect(&mut w).unwrap();
    }
    late_receiver.unprotect(&mut third).unwrap();
    // another key does not decrypt
    let mut other = Receiver::new(&MasterKey::random());
    let mut wire = packet(5, b"x");
    Sender::new(&master).protect(&mut wire).unwrap();
    assert_eq!(other.unprotect(&mut wire), Err(SrtpError::Auth));
}
