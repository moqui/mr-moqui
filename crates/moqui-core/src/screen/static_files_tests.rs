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
