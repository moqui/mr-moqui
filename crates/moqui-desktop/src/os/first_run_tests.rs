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

use super::{needs_pairing, signed_in_page, wizard_url};
use tauri::Url;

fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

#[test]
fn a_qapps_page_on_the_node_is_signed_in_and_the_login_or_wizard_is_not() {
    let node = url("http://127.0.0.1:43123/qapps");
    assert!(signed_in_page(&url("http://127.0.0.1:43123/qapps"), &node));
    assert!(signed_in_page(
        &url("http://127.0.0.1:43123/qapps/Comms/Inbox?x=1"),
        &node
    ));
    assert!(!signed_in_page(&url("http://127.0.0.1:43123/Login"), &node));
    assert!(!signed_in_page(&url("http://127.0.0.1:43123/qappsx"), &node));
    assert!(!signed_in_page(
        &url("http://127.0.0.1:43123/qapps/system/EdgeNode/Pair/EdgeUrl"),
        &node
    ));
    assert!(!signed_in_page(&url("http://127.0.0.1:9999/qapps"), &node));
    assert!(!signed_in_page(&url("http://tauri.localhost/index.html"), &node));
}

#[test]
fn the_wizard_is_on_the_node() {
    assert_eq!(
        wizard_url(&url("http://127.0.0.1:43123/qapps?a=b#c")).as_str(),
        "http://127.0.0.1:43123/qapps/system/EdgeNode/Pair"
    );
}

#[test]
fn a_paired_node_or_one_that_takes_links_needs_no_pairing() {
    let none = |_: &str| String::new();
    assert!(needs_pairing("", none));
    assert!(needs_pairing("# comment\nedge_node_url=\nnode_role=normal\n", none));
    assert!(!needs_pairing("edge_node_url=https://edge.example.com\n", none));
    assert!(!needs_pairing(
        "export edge_node_url=\"https://edge.example.com\"\n",
        none
    ));
    assert!(!needs_pairing("node_role=all-in-one\n", none));
    assert!(!needs_pairing("", |name: &str| if name == "edge_node_url" {
        "https://edge.example.com".to_string()
    } else {
        String::new()
    }));
    assert!(!needs_pairing("", |name: &str| if name == "node_role" {
        "edge".to_string()
    } else {
        String::new()
    }));
}
