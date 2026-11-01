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
fn host_names_are_normalized_and_others_refused() {
    assert_eq!(normalize(" ERP.Example.com ").as_deref(), Some("erp.example.com"));
    assert_eq!(normalize("erp.example.com:8443").as_deref(), Some("erp.example.com"));
    assert_eq!(normalize("erp.example.com.").as_deref(), Some("erp.example.com"));
    assert_eq!(normalize("localhost").as_deref(), Some("localhost"));
    for bad in [
        "",
        ":443",
        "[::1]:443",
        "a..b",
        ".a",
        "a b",
        "a/b",
        "*.example.com",
        &"a".repeat(254),
    ] {
        assert_eq!(normalize(bad), None, "{bad:?}");
    }
}

#[test]
fn a_route_is_found_by_any_spelling_of_its_name() {
    let hosts = Hosts::default();
    write(&hosts.table).1.insert(
        "erp.example.com".to_string(),
        Route {
            node: "local-1".to_string(),
            mode: Mode::Terminate,
        },
    );
    let route = hosts.route("ERP.example.com:443").unwrap();
    assert_eq!(route.node, "local-1");
    assert_eq!(route.mode, Mode::Terminate);
    assert!(hosts.route("other.example.com").is_none());
    assert!(hosts.route("").is_none());
    assert_eq!(Mode::from_enum(HOST_PASSTHROUGH), Some(Mode::Passthrough));
    assert_eq!(Mode::from_enum("EdgeHostOther"), None);
}
