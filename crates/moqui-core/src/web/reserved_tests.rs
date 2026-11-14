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
fn each_segment_is_listed_once_as_one_plain_segment() {
    let mut seen = std::collections::HashSet::new();
    for reserved in RESERVED {
        assert!(seen.insert(reserved.segment), "{} is listed twice", reserved.segment);
        assert!(
            !reserved.segment.is_empty() && !reserved.segment.contains('/'),
            "{:?} is not one segment",
            reserved.segment
        );
    }
    assert_eq!(reserved_paths().len(), RESERVED.len());
}

#[test]
fn the_host_routes_screens_and_static_files_are_reserved_as_written() {
    for first in ["git", "ci", "dav", "mcp", "apps", "rest", "Login", "sw.js", "libs"] {
        assert!(is_reserved(first), "{first}");
    }
    assert!(!is_reserved("login"), "matched as written");
    assert!(!is_reserved("forum"));
    assert!(is_static("libs") && !is_static("apps") && !is_static("git"));
}

#[test]
fn a_refusing_host_still_serves_git_and_the_ci_runner_rest_only() {
    assert!(served_on_refusing_host("/git/acme/widget.git/info/refs"));
    assert!(served_on_refusing_host("/git/acme/widget.git/info/lfs/objects/batch"));
    assert!(served_on_refusing_host("/ci/runner/v1/jobs/request"));
    for path in [
        "/git",
        "/ci/",
        "/ci/other",
        "/dav/",
        "/mcp",
        "/notws",
        "/health",
        "/gitx/a",
    ] {
        assert!(!served_on_refusing_host(path), "{path}");
    }
}
