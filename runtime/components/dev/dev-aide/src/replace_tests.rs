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

const MAX: u64 = 1 << 20;
const TEXT: &str = "<a name=\"orderId\"/>\n<b name=\"OrderId\" other=\"orderId\"/>\n<c/>\n";

#[test]
fn a_text_matches_ignoring_case_unless_asked_and_each_hit_has_its_line_before_and_after() {
    let replacer = Replacer::new("orderId", "salesOrderId", false, false).unwrap();
    let (hits, more) = replacer.hits(TEXT, 10);
    assert!(!more);
    let shown: Vec<(usize, usize, usize)> = hits.iter().map(|hit| (hit.index, hit.line, hit.column)).collect();
    assert_eq!(shown, vec![(0, 1, 10), (1, 2, 10), (2, 2, 26)]);
    assert_eq!(hits[1].text, "<b name=\"OrderId\" other=\"orderId\"/>");
    // only that one match replaced in its line's after
    assert_eq!(hits[1].after, "<b name=\"salesOrderId\" other=\"orderId\"/>");
    assert_eq!(hits[2].after, "<b name=\"OrderId\" other=\"salesOrderId\"/>");
    let exact = Replacer::new("orderId", "x", false, true).unwrap();
    assert_eq!(exact.hits(TEXT, 10).0.len(), 2);
    // a limit says there were more
    let (first, more) = replacer.hits(TEXT, 2);
    assert_eq!((first.len(), more), (2, true));
}

#[test]
fn only_the_chosen_matches_are_replaced() {
    let replacer = Replacer::new("orderId", "salesOrderId", false, false).unwrap();
    let (text, count) = replacer.replace(TEXT, &[0, 2].into_iter().collect(), MAX).unwrap();
    assert_eq!(count, 2);
    assert_eq!(
        text,
        "<a name=\"salesOrderId\"/>\n<b name=\"OrderId\" other=\"salesOrderId\"/>\n<c/>\n"
    );
    let (same, none) = replacer.replace(TEXT, &BTreeSet::new(), MAX).unwrap();
    assert_eq!((same.as_str(), none), (TEXT, 0));
    // a chosen index past the matches changes nothing
    assert_eq!(replacer.replace(TEXT, &[9].into_iter().collect(), MAX).unwrap().1, 0);
}

#[test]
fn a_regular_expression_expands_its_groups_and_a_text_does_not() {
    let regex = Replacer::new(r#"name="(\w+)""#, "name=\"${1}Ext\" was=\"$1\"", true, true).unwrap();
    let (text, count) = regex.replace(TEXT, &[0, 1].into_iter().collect(), MAX).unwrap();
    assert_eq!(count, 2);
    assert!(text.starts_with("<a name=\"orderIdExt\" was=\"orderId\"/>\n<b name=\"OrderIdExt\" was=\"OrderId\" other"));
    // in a text, $1 is two characters, and a regular expression's characters are themselves
    let plain = Replacer::new("a.b", "$1", false, true).unwrap();
    assert_eq!(
        plain.replace("a.b axb", &[0, 1].into_iter().collect(), MAX).unwrap(),
        ("$1 axb".to_string(), 1)
    );
}

#[test]
fn line_anchors_hold_in_a_crlf_file_and_a_match_over_lines_ends_its_line_there() {
    let regex = Replacer::new("Id\"/>$", "Id\" x=\"1\"/>", true, true).unwrap();
    let crlf = "<a id=\"orderId\"/>\r\n<b/>\r\n";
    let (hits, _) = regex.hits(crlf, 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].text, "<a id=\"orderId\"/>");
    assert_eq!(hits[0].after, "<a id=\"orderId\" x=\"1\"/>");
    let across = Replacer::new(r"/>\n<b", "/><B", true, true).unwrap();
    let (hits, _) = across.hits(TEXT, 10);
    assert_eq!((hits[0].line, hits[0].after.as_str()), (1, "<a name=\"orderId\"/><B"));
}

#[test]
fn nothing_to_find_a_bad_expression_and_one_that_matches_nothing_are_refused() {
    assert!(Replacer::new("", "x", false, true).is_err());
    assert!(Replacer::new("(", "x", true, true)
        .unwrap_err()
        .contains("regular expression"));
    assert!(Replacer::new("a*", "x", true, true).unwrap_err().contains("empty"));
    // the same characters as a text are fine
    assert!(Replacer::new("a*", "x", false, true).is_ok());
}

#[test]
fn a_group_number_ends_at_its_last_digit() {
    assert_eq!(numbered_groups("$1a ${x} $$1 $ $12b"), "${1}a ${x} $$1 $ ${12}b");
    let regex = Replacer::new(r"(\w+)=(\d)", "$1a=$2b", true, true).unwrap();
    assert_eq!(
        regex.replace("k=5", &[0].into_iter().collect(), MAX).unwrap().0,
        "ka=5b"
    );
}

#[test]
fn an_empty_match_is_passed_over_in_the_preview_and_the_replace() {
    // \b matches an empty text only between characters, so the refusal of an empty match does not catch it
    let regex = Replacer::new(r"x|\b", "Y", true, true).unwrap();
    let (hits, _) = regex.hits("ab x", 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].column, 4);
    assert_eq!(
        regex.replace("ab x", &[0].into_iter().collect(), MAX).unwrap(),
        ("ab Y".to_string(), 1)
    );
    assert!(!Replacer::new(r"\b", "Y", true, true).unwrap().finds("ab cd"));
}

#[test]
fn the_query_the_replacement_and_the_result_are_bounded() {
    assert!(Replacer::new(&"a".repeat(MAX_QUERY_BYTES + 1), "x", false, true)
        .unwrap_err()
        .contains("over"));
    assert!(Replacer::new("a", &"x".repeat(MAX_REPLACEMENT_BYTES + 1), false, true)
        .unwrap_err()
        .contains("over"));
    let big = Replacer::new("a", &"x".repeat(1000), false, true).unwrap();
    let all: BTreeSet<usize> = (0..10).collect();
    assert!(big
        .replace("aaaaaaaaaa", &all, 5000)
        .unwrap_err()
        .contains("over 5000 bytes"));
    // a long line is shown cut, with an ellipsis
    let long = format!("{}orderId{}", "x".repeat(1000), "y".repeat(1000));
    let (hits, _) = Replacer::new("orderId", "z", false, true).unwrap().hits(&long, 1);
    assert_eq!(hits[0].text.chars().count(), SHOWN_CHARS + 1);
    assert!(hits[0].text.ends_with('…') && hits[0].after.ends_with('…'));
}

#[test]
fn search_answers_by_line_what_replace_matches_over_the_whole_text() {
    let crlf = "<a/>\r\n<b id=\"x\"/>\r\n<c id=\"x\"/> <d id=\"x\"/>\n";
    // across lines, as Replace matches: the line a match starts on
    let across = compile(r"/>\r?\n<b", true, true).unwrap();
    assert_eq!(matched_lines(&across, crlf, 10), vec![(1, "<a/>".to_string())]);
    // a line with two matches is answered once; $ holds before a CRLF
    let ids = compile(r#"id="x"/>$"#, true, true).unwrap();
    assert_eq!(
        matched_lines(&ids, crlf, 10),
        vec![
            (2, "<b id=\"x\"/>".to_string()),
            (3, "<c id=\"x\"/> <d id=\"x\"/>".to_string())
        ]
    );
    assert_eq!(matched_lines(&compile("id=", false, true).unwrap(), crlf, 1).len(), 1);
    // an empty match is no match, in Search as in Replace
    assert!(matched_lines(&compile(r"\b", true, true).unwrap(), crlf, 10).is_empty());
}

#[test]
fn a_replacement_s_expansion_is_bounded_before_it_runs() {
    assert_eq!(group_refs("${1}a $name $$ $ ${x}"), 3);
    // each ${0} puts the whole match in again
    let many = "${0}".repeat(100);
    let regex = Replacer::new("x+", &many, true, true).unwrap();
    let text = format!("<{}>", "x".repeat(1000));
    // refused before the expansion, which could be 100 times the match
    assert!(regex
        .replace(&text, &[0].into_iter().collect(), 50_000)
        .unwrap_err()
        .contains("over 50000"));
    assert!(regex.replace("<xx>", &[0].into_iter().collect(), 50_000).is_ok());
    // a preview shows such a replacement as written, cut
    let (hits, _) = regex.hits(&text, 1);
    assert!(hits[0].after.chars().count() <= SHOWN_CHARS + 1);
    assert!(hits[0].after.contains("${0}"));
}
