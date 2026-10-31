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

fn local(port: u16, srtp: bool) -> LocalMedia {
    LocalMedia::new("192.0.2.10".parse().unwrap(), port, srtp.then(MasterKey::random))
}

#[test]
fn an_srtp_offer_and_its_answer_settle_the_codec_dtmf_and_keys() {
    let ours = local(20_000, true);
    let text = offer(&ours);
    assert!(text.contains("m=audio 20000 RTP/SAVP 0 8 111 101 110\r\n"), "{text}");
    assert!(text.contains("a=rtpmap:101 telephone-event/8000"));
    assert!(text.contains("a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:"));

    let theirs = local(30_000, true);
    let parsed = parse(&text).unwrap();
    assert!(parsed.secure);
    let (answer_text, settled) = answer(&parsed, &theirs).unwrap();
    assert_eq!(settled.codec.name, "pcmu");
    assert_eq!(settled.dtmf, Some((101, 8000)));
    assert_eq!(settled.remote, "192.0.2.10:20000".parse().unwrap());
    assert_eq!(settled.remote_key, ours.key);
    assert!(
        answer_text.contains("m=audio 30000 RTP/SAVP 0 101\r\n"),
        "{answer_text}"
    );

    let back = accept(&ours, &parse(&answer_text).unwrap()).unwrap();
    assert_eq!(back.codec.payload_type, 0);
    assert_eq!(back.remote_key, theirs.key);
    assert_eq!(back.direction, Direction::SendRecv);
}

#[test]
fn a_trunk_offering_opus_first_gets_opus_and_video_is_refused() {
    let offer_text = "v=0\r\no=- 1 1 IN IP4 198.51.100.7\r\ns=-\r\nc=IN IP4 198.51.100.7\r\nt=0 0\r\n\
        m=audio 4000 RTP/AVP 96 0 97\r\na=rtpmap:96 opus/48000/2\r\na=rtpmap:97 telephone-event/48000\r\n\
        m=video 4002 RTP/AVP 100\r\na=rtpmap:100 VP8/90000\r\na=sendonly\r\n";
    let parsed = parse(offer_text).unwrap();
    assert_eq!(
        parsed.direction,
        Direction::SendRecv,
        "the video's direction is not the audio's"
    );
    let (text, settled) = answer(&parsed, &local(20_002, false)).unwrap();
    assert_eq!(
        settled.codec,
        CodecChoice {
            name: "opus",
            payload_type: 96,
            clock_rate: 48_000
        }
    );
    assert_eq!(settled.dtmf, Some((97, 48_000)));
    assert!(text.contains("m=video 0 RTP/AVP 100"), "{text}");
}

#[test]
fn security_is_never_downgraded() {
    let plain = offer(&local(20_000, false));
    assert_eq!(
        answer(&parse(&plain).unwrap(), &local(1, true)).err(),
        Some(SdpError::Security)
    );
    let secure = offer(&local(20_000, true));
    assert_eq!(
        answer(&parse(&secure).unwrap(), &local(1, false)).err(),
        Some(SdpError::Security)
    );
    let keyless = secure
        .lines()
        .filter(|l| !l.starts_with("a=crypto"))
        .collect::<Vec<_>>()
        .join("\r\n");
    assert_eq!(
        answer(&parse(&keyless).unwrap(), &local(1, true)).err(),
        Some(SdpError::Security)
    );
    let g729 = "v=0\r\nc=IN IP4 198.51.100.7\r\nm=audio 4000 RTP/AVP 18\r\na=rtpmap:18 G729/8000\r\n";
    assert_eq!(
        answer(&parse(g729).unwrap(), &local(1, false)).err(),
        Some(SdpError::NoCodec)
    );
    assert_eq!(parse("not sdp").err(), Some(SdpError::NoAudio));
}

#[test]
fn hold_by_reinvite_is_mirrored_in_the_answer() {
    let mut holder = local(4_000, false);
    holder.direction = Direction::SendOnly;
    holder.version = 2;
    let (text, settled) = answer(&parse(&offer(&holder)).unwrap(), &local(20_000, false)).unwrap();
    assert!(text.contains("a=recvonly"));
    assert_eq!(settled.direction, Direction::RecvOnly, "held: this side stops sending");
    holder.direction = Direction::Inactive;
    let (text, settled) = answer(&parse(&offer(&holder)).unwrap(), &local(20_000, false)).unwrap();
    assert!(text.contains("a=inactive"));
    assert_eq!(settled.direction, Direction::Inactive);
    // the old form: c=0.0.0.0
    let old = "v=0\r\nc=IN IP4 0.0.0.0\r\nm=audio 4000 RTP/AVP 0\r\n";
    assert_eq!(parse(old).unwrap().direction, Direction::SendOnly);
    // resuming
    holder.direction = Direction::SendRecv;
    let (_, settled) = answer(&parse(&offer(&holder)).unwrap(), &local(20_000, false)).unwrap();
    assert_eq!(settled.direction, Direction::SendRecv);
}

#[test]
fn the_answer_names_the_offers_crypto_tag() {
    // an offer whose AES_CM_128_HMAC_SHA1_80 key is its second crypto line, tag 2
    let key = MasterKey::random();
    let offer_text = format!(
        "v=0\r\no=- 1 1 IN IP4 198.51.100.7\r\ns=-\r\nc=IN IP4 198.51.100.7\r\nt=0 0\r\nm=audio 4000 RTP/SAVP 0\r\n\
         a=crypto:1 AES_256_CM_HMAC_SHA1_80 inline:AAAA\r\na=crypto:2 AES_CM_128_HMAC_SHA1_80 inline:{}\r\n",
        key.inline()
    );
    let parsed = parse(&offer_text).unwrap();
    let (text, settled) = answer(&parsed, &local(20_004, true)).unwrap();
    assert_eq!(settled.remote_key, Some(key));
    assert!(text.contains("a=crypto:2 AES_CM_128_HMAC_SHA1_80 inline:"), "{text}");
}
