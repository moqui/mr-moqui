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

use super::{is_blocked, matches_tag, media_type, safe_segments};

fn names(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

#[test]
fn traversal_hidden_and_odd_names_are_refused() {
    assert!(safe_segments(&names(&["css", "moqui-html.css"])));
    for bad in [
        &["..", "etc", "passwd"][..],
        &["css", "..", "..", "x"],
        &["."],
        &[".hidden"],
        &["css", ".git", "config"],
        &["%2e%2e", "x"],
        &["css", "%2E%2e", "x"],
        &["a%2Fb"],
        &["a%5Cb"],
        &["a\\b"],
        &["a%00b"],
        &[""],
        &["c:", "x"],
        &["a%0Ab"],
    ] {
        assert!(!safe_segments(&names(bad)), "{bad:?}");
    }
    assert!(!safe_segments(&[]));
}

#[test]
fn sources_are_blocked_and_assets_are_not() {
    for name in [
        "a/Login.xml",
        "a/x.ftl",
        "a/Y.html.fancy",
        "a/z.groovy",
        "a/CONF.XML",
        "a/b.properties",
    ] {
        assert!(is_blocked(name), "{name}");
    }
    for name in ["a/x.css", "a/x.min.js", "a/readme", "a/xml", "a/font.woff2"] {
        assert!(!is_blocked(name), "{name}");
    }
}

#[test]
fn media_types_name_a_charset_for_text() {
    assert_eq!(media_type("a.css"), "text/css; charset=UTF-8");
    assert_eq!(media_type("a.min.js"), "text/javascript; charset=UTF-8");
    assert_eq!(media_type("a.WOFF2"), "font/woff2");
    assert_eq!(media_type("favicon.ico"), "image/x-icon");
    assert_eq!(media_type("noext"), "application/octet-stream");
}

#[test]
fn a_range_header_picks_one_span_and_ignores_what_it_cannot_answer() {
    use super::{byte_range, ByteRange};
    assert_eq!(byte_range("bytes=0-99", 1000), ByteRange::Span(0, 99));
    assert_eq!(byte_range("bytes=900-", 1000), ByteRange::Span(900, 999));
    assert_eq!(byte_range("bytes=990-2000", 1000), ByteRange::Span(990, 999));
    assert_eq!(byte_range("bytes=-100", 1000), ByteRange::Span(900, 999));
    assert_eq!(byte_range("bytes=-5000", 1000), ByteRange::Span(0, 999));
    assert_eq!(byte_range("Bytes = 5-5", 1000), ByteRange::Span(5, 5));
    assert_eq!(byte_range("bytes=1000-", 1000), ByteRange::Unsatisfiable);
    assert_eq!(byte_range("bytes=-0", 1000), ByteRange::Unsatisfiable);
    assert_eq!(byte_range("bytes=0-", 0), ByteRange::Unsatisfiable);
    for ignored in [
        "bytes=5-1",
        "bytes=0-1,5-9",
        "items=0-1",
        "bytes=a-b",
        "bytes=-",
        "bytes=1",
        "0-1",
    ] {
        assert_eq!(byte_range(ignored, 1000), ByteRange::Whole, "{ignored}");
    }
}

#[test]
fn if_none_match_compares_weakly_and_takes_a_list() {
    assert!(matches_tag("W/\"a-1\"", "W/\"a-1\""));
    assert!(matches_tag("\"a-1\"", "W/\"a-1\""));
    assert!(matches_tag("\"x\", W/\"a-1\"", "W/\"a-1\""));
    assert!(matches_tag("*", "W/\"a-1\""));
    assert!(!matches_tag("\"b\"", "W/\"a-1\""));
}

/// A source of `len` bytes (`offset % 251`) that counts the bytes read, with a strong or a weak tag.
struct FakeSource {
    len: u64,
    tag: Option<String>,
    read: std::sync::atomic::AtomicU64,
}

impl crate::resource::source::ByteSource for FakeSource {
    fn len(&self) -> u64 {
        self.len
    }
    fn tag(&self) -> Option<String> {
        self.tag.clone()
    }
    fn read_at(&self, offset: u64, max: usize) -> Result<Vec<u8>, moqui_lib::error::Error> {
        let end = self.len.min(offset.saturating_add(max as u64));
        self.read
            .fetch_add(end.saturating_sub(offset), std::sync::atomic::Ordering::SeqCst);
        Ok((offset..end).map(|at| (at % 251) as u8).collect())
    }
}

fn opened(
    len: u64,
    tag: Option<&str>,
    last_modified: Option<i64>,
) -> (crate::context::Opened, std::sync::Arc<FakeSource>) {
    let source = std::sync::Arc::new(FakeSource {
        len,
        tag: tag.map(str::to_string),
        read: Default::default(),
    });
    let opened = crate::context::Opened {
        stat: crate::context::ResourceStat {
            is_file: true,
            size: len,
            last_modified,
        },
        source: std::sync::Arc::clone(&source) as std::sync::Arc<dyn crate::resource::source::ByteSource>,
    };
    (opened, source)
}

fn conditional(method: &str, headers: &[(&str, &str)]) -> super::Conditional {
    let mut request = crate::web::WebRequest::new(method, "/f");
    for (name, value) in headers {
        request.set_header(*name, *value);
    }
    super::Conditional::of(&request)
}

/// The response `respond_opened` puts in a fresh web slot.
fn answer(request: &super::Conditional, opened: &crate::context::Opened) -> crate::web::WebResponse {
    let _web = crate::web::install(Default::default());
    super::respond_opened(request, Vec::new(), opened).unwrap();
    crate::web::take_response()
}

/// The bytes of a response, streamed or not.
fn body_of(response: &crate::web::WebResponse) -> Vec<u8> {
    match &response.stream {
        Some(stream) => {
            let length = usize::try_from(stream.length).unwrap();
            stream.source.read_at(stream.offset, length).unwrap()
        }
        None => response.body.clone(),
    }
}

fn expected(range: std::ops::Range<u64>) -> Vec<u8> {
    range.map(|at| (at % 251) as u8).collect()
}

/// One range reads only its bytes (`plans/archive/Maps.md` §5): a span, a suffix, a range past the end (416), several ranges
/// (the whole file), and a streamed body over the inline size that reads nothing until it is sent.
#[test]
fn a_range_is_answered_with_only_its_bytes() {
    let len = 1_000_000;
    let (file, source) = opened(len, Some("sha256-abc"), None);
    let response = answer(&conditional("GET", &[("range", "bytes=100-199")]), &file);
    assert_eq!(response.status, 206);
    assert_eq!(response.header("Content-Range"), Some("bytes 100-199/1000000"));
    assert_eq!(response.header("Content-Length"), Some("100"));
    assert_eq!(response.header("Accept-Ranges"), Some("bytes"));
    assert_eq!(response.header("ETag"), Some("\"sha256-abc\""));
    assert_eq!(response.body, expected(100..200));
    assert_eq!(
        source.read.load(std::sync::atomic::Ordering::SeqCst),
        100,
        "only the range is read"
    );

    let response = answer(&conditional("GET", &[("range", "bytes=-10")]), &file);
    assert_eq!(response.status, 206);
    assert_eq!(response.header("Content-Range"), Some("bytes 999990-999999/1000000"));
    assert_eq!(body_of(&response), expected(999_990..1_000_000));

    let response = answer(&conditional("GET", &[("range", "bytes=1000000-")]), &file);
    assert_eq!(response.status, 416);
    assert_eq!(response.header("Content-Range"), Some("bytes */1000000"));
    assert!(response.body.is_empty() && response.stream.is_none());

    let before = source.read.load(std::sync::atomic::Ordering::SeqCst);
    let response = answer(&conditional("GET", &[("range", "bytes=0-1,5-9")]), &file);
    assert_eq!(response.status, 200);
    assert_eq!(response.header("Content-Length"), Some("1000000"));
    assert!(response.header("Content-Range").is_none());
    let stream = response.stream.as_ref().expect("a large whole file streams");
    assert_eq!((stream.offset, stream.length), (0, len));
    assert_eq!(
        source.read.load(std::sync::atomic::Ordering::SeqCst),
        before,
        "a streamed body is read as it is sent"
    );
}

/// `If-Range` by strong comparison (RFC 9110 §13.1.5): the same strong tag gets the range; a weak tag, on either side,
/// another tag or a date gets the whole file.
#[test]
fn if_range_needs_the_same_strong_tag() {
    let (strong, _) = opened(1000, Some("sha256-abc"), None);
    let ranged = |file: &crate::context::Opened, if_range: &str| {
        answer(
            &conditional("GET", &[("range", "bytes=0-9"), ("if-range", if_range)]),
            file,
        )
        .status
    };
    assert_eq!(ranged(&strong, "\"sha256-abc\""), 206);
    assert_eq!(ranged(&strong, " \"sha256-abc\" "), 206);
    assert_eq!(
        ranged(&strong, "W/\"sha256-abc\""),
        200,
        "a weak tag never satisfies If-Range"
    );
    assert_eq!(ranged(&strong, "\"sha256-other\""), 200);
    assert_eq!(ranged(&strong, "Wed, 21 Oct 2026 07:28:00 GMT"), 200);
    // a file on disk has only a weak tag, which If-Range never matches, even presented back exactly
    let (weak, _) = opened(1000, None, Some(1_700_000_000_000));
    let tag = answer(&conditional("GET", &[]), &weak)
        .header("ETag")
        .unwrap()
        .to_string();
    assert!(tag.starts_with("W/"), "{tag}");
    assert_eq!(ranged(&weak, &tag), 200);
    assert!(super::if_range_holds("\"a\"", "\"a\""));
    assert!(!super::if_range_holds("W/\"a\"", "W/\"a\""));
}

/// `If-None-Match` compares weakly (a 304 with no body), and `HEAD` has the headers of the whole file, its `Range`
/// ignored, without a body.
#[test]
fn if_none_match_is_a_304_and_head_has_no_body() {
    let (file, source) = opened(1000, Some("sha256-abc"), None);
    let response = answer(&conditional("GET", &[("if-none-match", "W/\"sha256-abc\"")]), &file);
    assert_eq!(response.status, 304);
    assert!(response.body.is_empty());
    assert_eq!(source.read.load(std::sync::atomic::Ordering::SeqCst), 0);

    // a HEAD's Range is ignored (RFC 9110 §14.2): the headers of the whole file
    let response = answer(&conditional("HEAD", &[("range", "bytes=10-19")]), &file);
    assert_eq!(
        (response.status, response.header("Content-Length")),
        (200, Some("1000"))
    );
    assert!(response.header("Content-Range").is_none());
    assert!(response.body.is_empty() && response.stream.is_none());
    assert_eq!(
        source.read.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "HEAD reads nothing"
    );
}

/// A pinned revision (`web::send_pinned_response`): the recorded checksum is the strong `ETag` in place of the source's
/// weak one, so `If-Range` holds with it; the response is cached `private` for a day (never `no-store`, never shared),
/// a `.pmtiles` file as bytes to save; content whose strong tag or size is not the one recorded is refused (409), a
/// method other than GET and HEAD is 405, and a weak or quoted checksum is refused.
#[test]
fn a_pinned_revision_is_sent_only_while_it_is_the_content_recorded() {
    use crate::web::{send_pinned_response, FileDisposition, Pinned};
    let pinned = |file: &crate::context::Opened, method: &str, headers: &[(&str, &str)], checksum: &str, size: u64| {
        let _web = crate::web::install(Default::default());
        let mut request = crate::web::WebRequest::new(method, "/f");
        for (name, value) in headers {
            request.set_header(*name, *value);
        }
        crate::web::set_conditional(&request);
        let outcome = send_pinned_response(file, "us.pmtiles", FileDisposition::Inline, checksum, size).unwrap();
        (outcome, crate::web::take_response())
    };
    // a file on disk has no strong tag: the size is checked, and the checksum is the ETag
    let (weak, _) = opened(1000, None, Some(1_700_000_000_000));
    let tag = "sha256-0123abcd";
    let (outcome, response) = pinned(
        &weak,
        "GET",
        &[("range", "bytes=0-126"), ("if-range", "\"sha256-0123abcd\"")],
        tag,
        1000,
    );
    assert_eq!((outcome, response.status), (Pinned::Sent, 206));
    assert_eq!(response.header("ETag"), Some("\"sha256-0123abcd\""));
    assert_eq!(
        response.header("Cache-Control"),
        Some("private, max-age=86400, immutable")
    );
    assert_eq!(crate::web::pinned_cache_control(), "private, max-age=86400, immutable");
    assert_eq!(response.header("Accept-Ranges"), Some("bytes"));
    assert_eq!(response.header("X-Content-Type-Options"), Some("nosniff"));
    assert_eq!(response.header("Content-Type"), Some("application/octet-stream"));
    assert!(response
        .header("Content-Disposition")
        .unwrap()
        .starts_with("attachment"));
    assert_eq!(
        response
            .headers
            .iter()
            .filter(|(name, _)| name == "Cache-Control")
            .count(),
        1
    );
    assert_eq!(response.body, expected(0..127));
    let status = |file: &crate::context::Opened, method: &str, headers: &[(&str, &str)], checksum: &str, size: u64| {
        pinned(file, method, headers, checksum, size).1.status
    };
    assert_eq!(
        status(&weak, "GET", &[("if-none-match", "\"sha256-0123abcd\"")], tag, 1000),
        304
    );
    let weak_if_range = [("range", "bytes=0-126"), ("if-range", "W/\"sha256-0123abcd\"")];
    assert_eq!(status(&weak, "GET", &weak_if_range, tag, 1000), 200);
    assert_eq!(status(&weak, "GET", &[], tag, 999), 409, "another size");
    assert_eq!(status(&weak, "POST", &[], tag, 1000), 405);

    // a strong tag must be the one recorded: a stale checksum is refused, never sent under the old tag
    let (strong, source) = opened(1000, Some("sha256-new"), None);
    let (outcome, response) = pinned(&strong, "GET", &[("range", "bytes=0-9")], "sha256-old", 1000);
    assert_eq!((outcome, response.status), (Pinned::Changed, 409));
    assert!(response.header("ETag").is_none());
    assert_eq!(
        source.read.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "nothing of it is read"
    );
    assert_eq!(
        status(&strong, "GET", &[("range", "bytes=0-9")], "sha256-new", 1000),
        206
    );

    for bad in ["", "W/\"x\"", "W/x", "a\"b", "a b"] {
        let _web = crate::web::install(Default::default());
        assert!(
            send_pinned_response(&weak, "us.pmtiles", FileDisposition::Inline, bad, 1000).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn a_map_archive_is_binary() {
    assert_eq!(media_type("us.PMTiles"), "application/vnd.pmtiles");
}
