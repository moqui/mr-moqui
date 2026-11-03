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

use super::{base_for, url_on};
use tauri::Url;

#[test]
fn the_splash_is_on_the_bundles_pages_or_the_dev_server() {
    assert_eq!(
        url_on(&base_for(None), None).as_str(),
        "http://tauri.localhost/index.html"
    );
    let dev: Url = "http://127.0.0.1:1430".parse().unwrap();
    assert_eq!(
        url_on(&base_for(Some(&dev)), None).as_str(),
        "http://127.0.0.1:1430/index.html"
    );
}

#[test]
fn the_splash_carries_the_message() {
    let message = "The node ended (exit status: 1).\n\nno conf & 100% #broken?";
    let failed = url_on(&base_for(None), Some(message));
    assert_eq!(failed.path(), "/index.html");
    assert_eq!(failed.fragment(), None);
    let shown: Vec<_> = failed.query_pairs().collect();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].0, "failed");
    assert_eq!(shown[0].1, message);
}

#[test]
fn the_apps_pages_and_the_node_stay_in_the_window() {
    let dev: Url = "http://127.0.0.1:1430/".parse().unwrap();
    let bundled = base_for(None);
    let node: Url = "http://127.0.0.1:51234/qapps".parse().unwrap();
    let external = |url: &str, node: Option<&Url>, app: &Url| crate::is_external(&url.parse().unwrap(), node, app);
    assert!(
        !external("http://127.0.0.1:1430/index.html?failed=x", None, &dev),
        "the dev server's splash"
    );
    assert!(!external("http://127.0.0.1:1430/index.html", Some(&node), &dev));
    assert!(
        !external("http://tauri.localhost/index.html", None, &bundled),
        "the bundle's splash"
    );
    assert!(
        external("http://127.0.0.1:1430/index.html", None, &bundled),
        "a dev server port in a bundle"
    );
    assert!(
        !external("http://127.0.0.1:51234/qapps/x", Some(&node), &dev),
        "the node"
    );
    assert!(
        external("http://127.0.0.1:51235/", Some(&node), &dev),
        "another loopback port"
    );
    assert!(external("https://example.com/", Some(&node), &dev));
    assert!(external("mailto:a@example.com", Some(&node), &dev));
}
