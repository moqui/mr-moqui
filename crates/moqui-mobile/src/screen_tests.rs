// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

use super::*;

#[test]
fn the_servers_udp_candidate_is_taken_from_its_answer() {
    let sdp = "v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
        a=candidate:1 1 tcp 2130706431 10.1.0.202 10092 typ host tcptype passive\r\n\
        a=candidate:2 1 udp 2130706431 10.1.0.202 10091 typ host\r\n";
    assert_eq!(
        server_address(sdp),
        Some("10.1.0.202:10091".parse().expect("an address"))
    );
    assert_eq!(
        server_address("v=0\r\na=candidate:1 1 tcp 1 10.0.0.1 1 typ host\r\n"),
        None
    );
    assert_eq!(server_address("a=candidate:bad"), None);
}

#[test]
fn a_vp8_keyframe_is_told_by_its_frame_tag() {
    // RFC 6386 §9.1: bit 0 of the frame tag is 0 for a key frame
    assert!(is_vp8_keyframe(&[0x50, 0x2d, 0x01, 0x9d, 0x01, 0x2a]));
    assert!(!is_vp8_keyframe(&[0x31, 0x02, 0x00]));
    assert!(!is_vp8_keyframe(&[]));
}

#[test]
fn an_offer_is_one_send_only_vp8_video() {
    let sdp = offer().expect("an offer");
    stop();
    assert_eq!(sdp.matches("m=video").count(), 1);
    assert_eq!(sdp.matches("m=audio").count(), 0);
    assert!(sdp.contains("a=sendonly"));
    assert!(sdp.contains("VP8/90000"));
    assert!(!sdp.contains("a=candidate"));
}
