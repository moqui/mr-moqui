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

use tauri::http::{HeaderMap, HeaderValue};

use super::{document_request, has_script_policy};

fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.insert(*name, HeaderValue::from_static(value));
    }
    map
}

#[test]
fn a_navigation_is_a_document_and_a_fetch_is_not() {
    assert!(document_request(&headers(&[("sec-fetch-dest", "document")])));
    assert!(document_request(&headers(&[("sec-fetch-dest", "iframe")])));
    assert!(!document_request(&headers(&[
        ("sec-fetch-dest", "empty"),
        ("accept", "text/html,application/xhtml+xml")
    ])));
    assert!(document_request(&headers(&[(
        "accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
    )])));
    // loadComponent's
    assert!(!document_request(&headers(&[(
        "accept",
        "text/html,application/javascript,*/*"
    )])));
    assert!(!document_request(&headers(&[])));
}

#[test]
fn only_a_servers_script_policy_is_kept() {
    assert!(!has_script_policy("frame-ancestors 'none'; form-action 'self';"));
    assert!(has_script_policy(
        "frame-ancestors 'none'; script-src 'self' 'unsafe-eval'"
    ));
    assert!(has_script_policy("Default-Src 'self'"));
    assert!(!has_script_policy(""));
}
