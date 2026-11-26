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

//! A hosted root's paths and names (plans/design-dev-aide.md 4.10).

use super::{clean_path, id_of, location_of};

#[test]
fn a_hosted_path_stays_in_the_repository() {
    assert_eq!(clean_path("/a//b/c.xml").unwrap(), "a/b/c.xml");
    assert_eq!(clean_path("").unwrap(), "");
    for bad in ["../x", "a/./b", ".git/config", "a\\..\\b"] {
        assert!(clean_path(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_hosted_root_is_named_so_that_no_two_share_a_name() {
    // a repository id has no `/`, so the location of each repository and branch is its own
    assert_eq!(location_of("DOCS", "feature/new page"), "git://DOCS/feature/new page");
    assert_ne!(location_of("A-b", "c"), location_of("A", "b-c"));
    // the id is the row's sequenced id
    assert_eq!(id_of("100050"), "hosted-100050");
}
