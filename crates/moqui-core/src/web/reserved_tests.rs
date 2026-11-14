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
    for partly in PARTLY_RESERVED {
        assert!(seen.insert(partly.segment), "{} is listed twice", partly.segment);
    }
    assert_eq!(reserved_paths().len(), RESERVED.len() + PARTLY_RESERVED.len());
}

#[test]
fn the_host_routes_screens_and_static_files_are_reserved_as_written() {
    for first in ["git", "dav", "mcp", "apps", "rest", "Login", "sw.js", "libs"] {
        assert!(is_reserved(first), "{first}");
    }
    for partly in PARTLY_RESERVED {
        assert!(!is_reserved(partly.segment), "{} is in one list only", partly.segment);
    }
    assert!(!is_reserved("login"), "matched as written");
    assert!(!is_reserved("forum"));
    assert!(is_static("libs") && !is_static("apps") && !is_static("git"));
}

/// `.well-known` and `ci` are reserved only under the host's own sub-paths: a site serves `/.well-known/security.txt`
/// and a page at `/ci`, never `/ci/runner/` or `/.well-known/caldav`. As names (a group, a mount) they stay reserved
/// whole, and `ec.web.reservedPaths` lists them; `reservedSubPaths` lists the sub-paths.
#[test]
fn well_known_and_ci_are_reserved_per_sub_path_for_pages_and_whole_for_names() {
    for path in [
        "/.well-known/caldav",
        "/.well-known/carddav",
        "/.well-known/agent-card.json",
        "/.well-known/mcp.json",
        "/.well-known/ai-catalog.json",
        "/ci/runner/jobs/j1/log",
        "/ci/runner",
        "/git/acme/widget.git/info/refs",
        "/apps/x",
        "/Login",
    ] {
        assert!(is_reserved_path(path), "{path}");
    }
    for path in [
        "/.well-known/security.txt",
        "/.well-known/apple-app-site-association",
        "/.well-known/assetlinks.json",
        "/.well-known",
        "/ci",
        "/ci/",
        "/ci/pipelines",
        "/forum/x",
        "/login",
    ] {
        assert!(!is_reserved_path(path), "{path}");
    }
    for name in ["ci", ".well-known", "git", "apps"] {
        assert!(is_reserved_name(name), "{name}");
    }
    assert!(!is_reserved_name("forum"));
    let paths = reserved_paths();
    assert!(paths.contains(&"ci") && paths.contains(&".well-known"));
    assert_eq!(paths.len(), RESERVED.len() + PARTLY_RESERVED.len());
    let subs = reserved_sub_paths();
    assert!(subs.iter().any(|sub| sub == "ci/runner") && subs.iter().any(|sub| sub == ".well-known/caldav"));
    assert!(!subs.iter().any(|sub| sub.contains("security.txt")));
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
