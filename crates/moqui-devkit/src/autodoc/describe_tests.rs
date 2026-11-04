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

const LICENSE: &str =
    "<!--\nThis software is in the public domain under CC0 1.0 Universal plus a\nGrant of Patent License.\n-->\n";

#[test]
fn each_elements_description_is_kept_by_its_line() {
    let text = format!(
        "<?xml version=\"1.0\"?>\n{LICENSE}<entities>\n    <entity entity-name=\"A\" package=\"p\">\n        <description>The first\n            line goes on.\n\n            A second paragraph.</description>\n        <field name=\"x\" type=\"id\"><description>An x.</description></field>\n        <field name=\"y\" type=\"id\"/>\n    </entity>\n</entities>\n"
    );
    let docs = read_text("test", &text).unwrap();
    assert_eq!(
        docs.by_line.get(&7).map(String::as_str),
        Some("The first line goes on.\n\nA second paragraph.")
    );
    assert_eq!(docs.by_line.get(&12).map(String::as_str), Some("An x."));
    assert_eq!(docs.by_line.get(&13), None);
    assert_eq!(docs.lead, None, "only a license comment");
}

#[test]
fn the_lead_comment_leaves_the_license_out() {
    let text =
        format!("<?xml version=\"1.0\"?>\n{LICENSE}<!-- The order screens:\n     find and detail. -->\n<screen/>\n");
    assert_eq!(
        lead_comment(&text).as_deref(),
        Some("The order screens: find and detail.")
    );
    // a license comment left open, holding the next comment's start (as some screens have it)
    let open = "<!--\nThis software is in the public domain under CC0.\n<http://creativecommons.org/publicdomain/zero/1.0/>.\n<!-- The Artifact Browser. -->\n<screen/>";
    assert_eq!(lead_comment(open).as_deref(), Some("The Artifact Browser."));
    // no comment before the root: the first in it, before its first element
    let inside =
        "<services>\n    <!-- Order services. -->\n    <service verb=\"a\"/>\n    <!-- not this -->\n</services>";
    assert_eq!(lead_comment(inside).as_deref(), Some("Order services."));
    assert_eq!(
        lead_comment("<services><service verb=\"a\"/><!-- late --></services>"),
        None
    );
}

#[test]
fn a_run_reads_each_file_once_and_tolerates_one_that_does_not_parse() {
    let dir = std::env::temp_dir().join(format!("autodoc-describe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let good = dir.join("good.xml");
    let bad = dir.join("bad.xml");
    std::fs::write(
        &good,
        "<services>\n<service verb=\"a\"><description>Does a.</description></service>\n</services>",
    )
    .unwrap();
    std::fs::write(&bad, "<services><service></services>").unwrap();
    let mut descriptions = Descriptions::default();
    let good_path = good.display().to_string();
    assert_eq!(descriptions.at(&good_path, 2).as_deref(), Some("Does a."));
    std::fs::write(&good, "<services/>").unwrap();
    assert_eq!(
        descriptions.at(&good_path, 2).as_deref(),
        Some("Does a."),
        "read once a run"
    );
    assert_eq!(descriptions.at(&bad.display().to_string(), 1), None);
    assert_eq!(descriptions.lead("/no/such/file.xml"), None);
    std::fs::remove_dir_all(&dir).unwrap();
}
