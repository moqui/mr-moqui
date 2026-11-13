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

use super::page_request;

fn on_site(method: &str, path: &str, query: Option<&str>) -> Option<String> {
    page_request(method, path, query, "code.test", &|host| host == "code.test")
}

#[test]
fn git_requests_at_a_page_url_are_served_under_git() {
    let upload = Some("service=git-upload-pack");
    assert_eq!(
        on_site("GET", "/moqui/mr-moqui/info/refs", upload).as_deref(),
        Some("/git/moqui/mr-moqui/info/refs")
    );
    assert_eq!(
        on_site(
            "GET",
            "/moqui/mr-moqui.git/info/refs",
            Some("a=b&service=git-receive-pack")
        )
        .as_deref(),
        Some("/git/moqui/mr-moqui.git/info/refs")
    );
    for service in ["git-upload-pack", "git-receive-pack"] {
        assert_eq!(
            on_site("POST", &format!("/moqui/mr-moqui/{service}"), None),
            Some(format!("/git/moqui/mr-moqui/{service}"))
        );
    }
    for (method, tail) in [
        ("POST", "objects/batch"),
        ("GET", "objects/abc"),
        ("PUT", "objects/abc"),
        ("POST", "locks/7/unlock"),
    ] {
        let path = format!("/moqui/mr-moqui.git/info/lfs/{tail}");
        assert_eq!(
            on_site(method, &path, None),
            Some(format!("/git{path}")),
            "{method} {tail}"
        );
    }
}

#[test]
fn every_other_request_is_the_sites_page() {
    let upload = Some("service=git-upload-pack");
    // not the smart protocol's: no service, another one, the wrong method, no suffix
    assert_eq!(on_site("GET", "/moqui/mr-moqui/info/refs", None), None);
    assert_eq!(
        on_site("GET", "/moqui/mr-moqui/info/refs", Some("service=git-upload-archive")),
        None
    );
    assert_eq!(on_site("POST", "/moqui/mr-moqui/info/refs", upload), None);
    assert_eq!(on_site("GET", "/moqui/mr-moqui/git-upload-pack", None), None);
    assert_eq!(on_site("GET", "/moqui/mr-moqui", upload), None);
    assert_eq!(
        on_site("GET", "/moqui/mr-moqui/info/lfs/objects/abc", None),
        None,
        "LFS has .git"
    );
    // the first segment is the framework's
    for first in ["git", "rest", "apps", "qapps", "Login", "static"] {
        assert_eq!(
            on_site("GET", &format!("/{first}/mr-moqui/info/refs"), upload),
            None,
            "{first}"
        );
        assert_eq!(
            on_site("POST", &format!("/{first}/x/git-upload-pack"), None),
            None,
            "{first}"
        );
        assert_eq!(
            on_site("POST", &format!("/{first}/x.git/info/lfs/objects/batch"), None),
            None,
            "{first}"
        );
    }
    // a section of a page, and what is not a repository path
    assert_eq!(on_site("GET", "/moqui/mr-moqui/-/tree/main/info/refs", upload), None);
    assert_eq!(on_site("POST", "/moqui/-/issues/git-upload-pack", None), None);
    assert_eq!(on_site("GET", "/moqui/.hidden/info/refs", upload), None);
    assert_eq!(on_site("GET", "/info/refs", upload), None);
    assert_eq!(on_site("GET", "/a/b/c/d/e/f/g/h/i/info/refs", upload), None);
    // a host that is no site's
    let other = page_request("GET", "/moqui/mr-moqui/info/refs", upload, "apps.test", &|host| {
        host == "code.test"
    });
    assert_eq!(other, None);
}
