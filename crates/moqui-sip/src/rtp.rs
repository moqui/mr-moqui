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

//! The RTP fixed header (RFC 3550 §5.1), what a phone leg needs of it: the payload type, sequence number, timestamp,
//! SSRC and marker, past any CSRCs, header extension and padding.

/// An RTP header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpHeader {
    pub marker: bool,
    pub payload_type: u8,
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
}

/// Where a packet's parts are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parsed {
    pub header: RtpHeader,
    /// The length of the header, CSRCs and extension: where the payload starts.
    pub header_len: usize,
    /// Where the payload ends (before any padding).
    pub payload_end: usize,
}

/// Whether a datagram is RTCP (RFC 5761 §4: packet types 192–223 in the second byte), not RTP.
pub fn is_rtcp(packet: &[u8]) -> bool {
    packet.len() >= 2 && (192..=223).contains(&packet[1])
}

/// The header of an RTP packet; `None` for anything else.
pub fn parse(packet: &[u8]) -> Option<Parsed> {
    let [first, second, s0, s1, t0, t1, t2, t3, c0, c1, c2, c3, ..] = packet else {
        return None;
    };
    if first >> 6 != 2 || is_rtcp(packet) {
        return None;
    }
    let csrcs = usize::from(first & 0x0f);
    let mut header_len = 12 + 4 * csrcs;
    if first & 0x10 != 0 {
        let ext = packet.get(header_len..header_len + 4)?;
        header_len += 4 + 4 * usize::from(u16::from_be_bytes([ext[2], ext[3]]));
    }
    let mut payload_end = packet.len();
    if first & 0x20 != 0 {
        let pad = usize::from(*packet.last()?);
        payload_end = payload_end.checked_sub(pad)?;
    }
    if header_len > payload_end {
        return None;
    }
    Some(Parsed {
        header: RtpHeader {
            marker: second & 0x80 != 0,
            payload_type: second & 0x7f,
            sequence: u16::from_be_bytes([*s0, *s1]),
            timestamp: u32::from_be_bytes([*t0, *t1, *t2, *t3]),
            ssrc: u32::from_be_bytes([*c0, *c1, *c2, *c3]),
        },
        header_len,
        payload_end,
    })
}

/// Writes a packet with no CSRCs, extension or padding.
pub fn write(header: &RtpHeader, payload: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.push(0x80);
    out.push((u8::from(header.marker) << 7) | (header.payload_type & 0x7f));
    out.extend_from_slice(&header.sequence.to_be_bytes());
    out.extend_from_slice(&header.timestamp.to_be_bytes());
    out.extend_from_slice(&header.ssrc.to_be_bytes());
    out.extend_from_slice(payload);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_packet_round_trips_and_skips_extensions_and_padding() {
        let header = RtpHeader {
            marker: true,
            payload_type: 0,
            sequence: 65_535,
            timestamp: 0xdead_beef,
            ssrc: 0x1234_5678,
        };
        let mut out = Vec::new();
        write(&header, &[1, 2, 3], &mut out);
        let parsed = parse(&out).unwrap();
        assert_eq!(parsed.header, header);
        assert_eq!(&out[parsed.header_len..parsed.payload_end], &[1, 2, 3]);
        // one CSRC, a one-word extension and two bytes of padding
        let mut packet = vec![0x80 | 0x20 | 0x10 | 1, 8, 0, 1, 0, 0, 0, 160, 0, 0, 0, 7];
        packet.extend_from_slice(&[0, 0, 0, 9]);
        packet.extend_from_slice(&[0xbe, 0xde, 0, 1, 1, 2, 3, 4]);
        packet.extend_from_slice(&[0xaa, 0xbb, 0, 2]);
        let parsed = parse(&packet).unwrap();
        assert_eq!(parsed.header.payload_type, 8);
        assert_eq!(&packet[parsed.header_len..parsed.payload_end], &[0xaa, 0xbb]);
        assert!(parse(&[0x80, 200, 0, 1]).is_none());
        assert!(is_rtcp(&[0x80, 200, 0, 6]));
        assert!(parse(&[0x40; 20]).is_none(), "not version 2");
    }
}
