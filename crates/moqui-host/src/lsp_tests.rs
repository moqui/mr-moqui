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

//! The tests of `moqui lsp` (lsp.rs).

use super::*;

#[test]
fn uris_and_paths_both_ways() {
    let path = Path::new("/w/my site/screen/A.xml");
    assert_eq!(uri_of(path), "file:///w/my%20site/screen/A.xml");
    assert_eq!(path_of(&uri_of(path)).unwrap(), path);
    assert_eq!(path_of("file:///C:/w/a.xml").unwrap(), PathBuf::from("C:/w/a.xml"));
    assert!(path_of("untitled:1").is_none());
    // a first character of several bytes before a colon is no drive letter's slash, and is kept
    assert_eq!(path_of("file://\u{e9}:/a.xml").unwrap(), PathBuf::from("\u{e9}:/a.xml"));
    // a % before a character of several bytes is taken as it is, not sliced
    assert_eq!(
        path_of("file:///w/%a\u{e9}.xml").unwrap(),
        PathBuf::from("/w/%a\u{e9}.xml")
    );
}

#[test]
fn the_word_and_the_tag_under_the_cursor() {
    let text = "<screen>\n    <service-call name=\"mantle.order.OrderServices.place#Order\" in-map=\"\"/>\n    <entity-find ";
    assert_eq!(word_at(text, 1, 30), "mantle.order.OrderServices.place#Order");
    let context = |element: &str, place: TagPlace| {
        Some(TagContext {
            element: element.into(),
            place,
        })
    };
    assert_eq!(
        tag_context(text, 1, 30),
        context("service-call", TagPlace::Value("name".into()))
    );
    assert_eq!(tag_context(text, 2, 17), context("entity-find", TagPlace::Attributes));
    assert_eq!(tag_context("<screen>\n    <ent", 1, 8), context("ent", TagPlace::Name));
    assert_eq!(tag_context("<screen>text", 0, 10), None);
}

#[test]
fn a_flag_without_its_value_is_the_usage_error() {
    for flag in ["--server", "--java", "--project"] {
        let err = dispatch_lsp(&[flag.to_string()]).unwrap_err().to_string();
        assert!(err.contains("moqui help lsp"), "{flag}: {err}");
    }
}
