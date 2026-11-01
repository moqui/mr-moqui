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

//! The STUN keepalive of a UDP phone (RFC 5626 §4.4.2, §8.4): a phone using SIP Outbound over UDP sends a STUN Binding
//! request (RFC 8489) on its SIP flow now and then, and takes the answer's mapped address to see whether its NAT binding
//! changed. Only the Binding request is answered here, with no authentication (none is defined for this use), its
//! transaction id and an XOR-MAPPED-ADDRESS of the address it came from; anything else that is not SIP is left alone.

use std::net::{IpAddr, SocketAddr};

/// The STUN magic cookie.
const COOKIE: u32 = 0x2112_A442;
const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const XOR_MAPPED_ADDRESS: u16 = 0x0020;
const HEADER: usize = 20;

/// Whether `packet` is a STUN Binding request: the first two bits zero, the method and class of one, the magic cookie, and
/// a length that is the rest of the datagram (a multiple of four).
pub fn is_binding_request(packet: &[u8]) -> bool {
    if packet.len() < HEADER || packet[0] & 0xC0 != 0 {
        return false;
    }
    let kind = u16::from_be_bytes([packet[0], packet[1]]);
    let length = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
    let cookie = u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]);
    kind == BINDING_REQUEST && cookie == COOKIE && length % 4 == 0 && HEADER + length == packet.len()
}

/// The Binding success answer to a Binding `request` from `from`: its transaction id and the address it came from, as an
/// XOR-MAPPED-ADDRESS. None when `request` is not a Binding request.
pub fn binding_response(request: &[u8], from: SocketAddr) -> Option<Vec<u8>> {
    if !is_binding_request(request) {
        return None;
    }
    let transaction = &request[8..HEADER];
    let port = from.port() ^ (COOKIE >> 16) as u16;
    // an IPv4 phone on a dual-stack socket is answered with its IPv4 address, as it sees itself
    let (family, address): (u8, Vec<u8>) = match from.ip().to_canonical() {
        IpAddr::V4(v4) => (0x01, (u32::from(v4) ^ COOKIE).to_be_bytes().to_vec()),
        IpAddr::V6(v6) => {
            let mut mask = COOKIE.to_be_bytes().to_vec();
            mask.extend_from_slice(transaction);
            (0x02, v6.octets().iter().zip(mask).map(|(a, m)| a ^ m).collect())
        }
    };
    let value_len = 4 + address.len();
    let mut out = Vec::with_capacity(HEADER + 4 + value_len);
    out.extend_from_slice(&BINDING_SUCCESS.to_be_bytes());
    out.extend_from_slice(&u16::try_from(4 + value_len).ok()?.to_be_bytes());
    out.extend_from_slice(&COOKIE.to_be_bytes());
    out.extend_from_slice(transaction);
    out.extend_from_slice(&XOR_MAPPED_ADDRESS.to_be_bytes());
    out.extend_from_slice(&u16::try_from(value_len).ok()?.to_be_bytes());
    out.push(0);
    out.push(family);
    out.extend_from_slice(&port.to_be_bytes());
    out.extend_from_slice(&address);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(transaction: [u8; 12]) -> Vec<u8> {
        let mut out = vec![0x00, 0x01, 0x00, 0x00];
        out.extend_from_slice(&COOKIE.to_be_bytes());
        out.extend_from_slice(&transaction);
        out
    }

    #[test]
    fn a_binding_request_is_answered_with_its_mapped_address_and_nothing_else_is() {
        let transaction = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let from: SocketAddr = "192.0.2.1:32853".parse().unwrap();
        let answer = binding_response(&request(transaction), from).expect("an answer");
        // RFC 5769 §2.2's mapped address, as the sample response carries it
        assert_eq!(&answer[0..2], &[0x01, 0x01]);
        assert_eq!(&answer[2..4], &[0x00, 0x0c]);
        assert_eq!(&answer[8..20], &transaction);
        assert_eq!(&answer[20..24], &[0x00, 0x20, 0x00, 0x08]);
        assert_eq!(&answer[24..32], &[0x00, 0x01, 0xa1, 0x47, 0xe1, 0x12, 0xa6, 0x43]);
        let v6: SocketAddr = "[2001:db8:1234:5678:11:2233:4455:6677]:32853".parse().unwrap();
        let answer = binding_response(&request(transaction), v6).expect("an answer");
        assert_eq!(answer.len(), 20 + 4 + 20);
        assert_eq!(answer[25], 0x02);
        // an IPv4 phone seen on a dual-stack socket: its IPv4 address
        let mapped: SocketAddr = "[::ffff:192.0.2.1]:32853".parse().unwrap();
        assert_eq!(
            binding_response(&request(transaction), mapped),
            binding_response(&request(transaction), from)
        );
        // not STUN: SIP, a CRLF keepalive, a Binding answer, a request with a wrong length
        assert!(binding_response(b"OPTIONS sip:a@b SIP/2.0\r\n\r\n", from).is_none());
        assert!(binding_response(b"\r\n\r\n", from).is_none());
        let mut success = request(transaction);
        success[1] = 0x01;
        success[0] = 0x01;
        assert!(binding_response(&success, from).is_none());
        let mut long = request(transaction);
        long[3] = 4;
        assert!(binding_response(&long, from).is_none());
    }
}
