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
fn a_stream_is_read_in_pieces_and_stanzas_come_out_whole() {
    let mut reader = StreamReader::new(1024);
    let text =
        "<?xml version='1.0'?><stream:stream xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams' \
        to='moqui.local' version='1.0'> <message to='a@b'><body>hi &amp; bye</body></message>\n \
        <iq type='get' id='1'><ping xmlns='urn:xmpp:ping'/></iq></stream:stream>";
    let mut frames = Vec::new();
    for chunk in text.as_bytes().chunks(7) {
        frames.extend(reader.feed(chunk).unwrap());
    }
    assert!(matches!(&frames[0], Frame::Open(attrs) if attrs.get("to").map(String::as_str) == Some("moqui.local")));
    let Frame::Element(message) = &frames[1] else {
        panic!("a stanza")
    };
    assert!(message.is("message", ns::CLIENT));
    assert_eq!(message.get_child("body", ns::CLIENT).unwrap().text(), "hi & bye");
    let Frame::Element(iq) = &frames[2] else {
        panic!("a stanza")
    };
    assert!(iq.has_child("ping", ns::PING));
    assert!(matches!(frames[3], Frame::Close));
    assert_eq!(frames.len(), 4);
}

#[test]
fn entities_and_oversized_stanzas_are_refused() {
    let mut reader = StreamReader::new(1024);
    let refused = reader.feed(b"<?xml version='1.0'?><!DOCTYPE x [<!ENTITY a 'b'>]><x/>");
    assert!(matches!(refused, Err(FramingError::NotWellFormed(_))));
    let mut reader = StreamReader::new(128);
    reader
        .feed(b"<stream:stream xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams'><message>")
        .unwrap();
    assert_eq!(reader.feed(&[b'a'; 200]).err(), Some(FramingError::TooLarge));
}

#[test]
fn a_deep_stanza_and_a_long_header_are_refused_before_they_are_built() {
    // nested elements past the limit: minidom would overflow the stack dropping them
    let mut reader = StreamReader::new(256 * 1024);
    reader
        .feed(b"<stream:stream xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams'>")
        .unwrap();
    let deep = format!("<message>{}", "<a>".repeat(40_000));
    assert_eq!(reader.feed(deep.as_bytes()).err(), Some(FramingError::TooDeep));
    let frame = format!("<message xmlns='jabber:client'>{}</message>", "<a/>".repeat(3));
    assert!(parse_frame(&frame).is_ok());
    let deep = format!("<a xmlns='x'>{}{}</a>", "<a>".repeat(40_000), "</a>".repeat(40_000));
    assert_eq!(parse_frame(&deep).err(), Some(FramingError::TooDeep));
    let nested = format!(
        "<a xmlns='x'>{}{}</a>",
        "<a>".repeat(MAX_DEPTH - 1),
        "</a>".repeat(MAX_DEPTH - 1)
    );
    assert!(parse_element(&nested).is_ok());
    // a stream header that never ends counts toward the limit too
    let mut reader = StreamReader::new(1024);
    let mut header =
        String::from("<stream:stream xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams'");
    for i in 0..200 {
        header.push_str(&format!(" a{i}='x'"));
    }
    assert_eq!(reader.feed(header.as_bytes()).err(), Some(FramingError::TooLarge));
    // keepalives between stanzas do not add up
    let mut reader = StreamReader::new(128);
    reader
        .feed(b"<stream:stream xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams'>")
        .unwrap();
    for _ in 0..200 {
        reader.feed(b" ").unwrap();
    }
    assert!(reader.feed(b"<presence/>").is_ok());
}

/// SEC-COM-82: a stanza's start tag sent as attributes, each value in a read of its own that ends inside it, counts
/// every byte, so the stream ends at the limit; under the limit the same tag, closed, is a stanza.
#[test]
fn a_start_tag_cut_inside_its_attributes_counts_toward_the_limit() {
    let header = b"<stream:stream xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams'>";
    let value = "v".repeat(1000);
    let mut reader = StreamReader::new(16 * 1024);
    reader.feed(header).unwrap();
    reader.feed(b"<iq a0='").unwrap();
    let mut refused = None;
    for index in 1..40 {
        let fed = reader
            .feed(value.as_bytes())
            .and_then(|_| reader.feed(format!("' a{index}='").as_bytes()));
        match fed {
            Ok(frames) => assert!(frames.is_empty()),
            Err(err) => {
                refused = Some((index, err));
                break;
            }
        }
    }
    let (index, err) = refused.expect("the start tag passes the limit");
    assert_eq!(err, FramingError::TooLarge);
    assert!(index <= 17, "refused only after {index} KB");
    // the same pieces, fewer of them: under the limit the stanza comes whole
    let mut reader = StreamReader::new(16 * 1024);
    reader.feed(header).unwrap();
    reader.feed(b"<iq a0='").unwrap();
    for index in 1..14 {
        assert!(reader.feed(value.as_bytes()).unwrap().is_empty());
        assert!(reader.feed(format!("' a{index}='").as_bytes()).unwrap().is_empty());
    }
    let frames = reader.feed(b"x'/>").unwrap();
    assert!(matches!(&frames[..], [Frame::Element(iq)] if iq.attr("a13") == Some("x")));
    // and keepalives after it are still not counted
    for _ in 0..200 {
        reader.feed(b" ").unwrap();
    }
    assert!(reader.feed(b"<presence/>").is_ok());
}

#[test]
fn websocket_frames_are_whole_elements() {
    let open =
        parse_frame("<open xmlns='urn:ietf:params:xml:ns:xmpp-framing' to='moqui.local' version='1.0'/>").unwrap();
    assert!(matches!(open, Frame::Open(attrs) if attrs["to"] == "moqui.local"));
    assert!(matches!(parse_frame(CLOSE_FRAME).unwrap(), Frame::Close));
    let Frame::Element(presence) = parse_frame("<presence xmlns='jabber:client'/>").unwrap() else {
        panic!("a stanza")
    };
    assert!(presence.is("presence", ns::CLIENT));
    assert!(write(&presence).contains("jabber:client"));
}
