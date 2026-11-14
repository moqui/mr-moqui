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

fn pairing(facade: &EntityFacade, node: &str, status: &str) {
    let mut client = facade.make_value(CLIENT).unwrap();
    client.set("pairedNodeId", Value::string(node));
    client.set("keyFingerprint", Value::string("ab"));
    client.set("statusId", Value::string(status));
    client.create(facade).unwrap();
}

fn host(name: &str, node: &str, mode: &str) -> EdgeHostInfo {
    EdgeHostInfo {
        host_name: name.to_string(),
        paired_node_id: node.to_string(),
        mode_enum_id: mode.to_string(),
        sticky_cookie: None,
    }
}

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

/// A name goes to one pairing: the first claim holds, and another pairing's add is refused with the generic text; the
/// same pairing changes its mode; a revoked pairing, a bad name and a bad mode are refused; remove says whether there
/// was a name.
#[test]
fn a_host_name_has_one_pairing_and_the_first_claim_holds() {
    let facade = crate::testing::open();
    pairing(&facade, "local-a", ACTIVE);
    pairing(&facade, "local-b", ACTIVE);
    pairing(&facade, "local-c", "EdgeClientRevoked");
    let saved = add(&facade, &host("ERP.Example.com:443", "local-a", HOST_PASSTHROUGH)).unwrap();
    assert_eq!(saved.host_name, "erp.example.com");
    let err = add(&facade, &host("erp.example.com", "local-b", HOST_TERMINATE)).unwrap_err();
    assert_eq!(err.message(), "erp.example.com: another node has this name");
    assert!(!err.message().contains("local-a"), "the holder is not named");
    let changed = add(&facade, &host("erp.example.com", "local-a", HOST_TERMINATE)).unwrap();
    assert_eq!(changed.mode_enum_id, HOST_TERMINATE);
    assert!(add(&facade, &host("shop.example.com", "local-c", HOST_PASSTHROUGH))
        .unwrap_err()
        .message()
        .contains("revoked"));
    assert!(add(&facade, &host("shop.example.com", "local-x", HOST_PASSTHROUGH)).is_err());
    assert!(add(&facade, &host("a b", "local-a", HOST_PASSTHROUGH)).is_err());
    assert!(add(&facade, &host("shop.example.com", "local-a", "Other")).is_err());
    add(&facade, &host("shop.example.com", "local-b", HOST_PASSTHROUGH)).unwrap();
    let all = list(&facade, None).unwrap();
    assert_eq!(
        all.iter()
            .map(|host| (
                host.host_name.as_str(),
                host.paired_node_id.as_str(),
                host.mode_enum_id.as_str()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("erp.example.com", "local-a", HOST_TERMINATE),
            ("shop.example.com", "local-b", HOST_PASSTHROUGH)
        ]
    );
    assert_eq!(list(&facade, Some("local-b")).unwrap().len(), 1);
    assert!(remove(&facade, "SHOP.example.com").unwrap());
    assert!(!remove(&facade, "shop.example.com").unwrap(), "gone");
    assert!(!remove(&facade, "not a name").unwrap());
}

/// Two claims of a new name at once: the second read no row, and its insert finds the first's, so it is refused
/// rather than taking the name.
#[test]
fn a_claim_that_read_no_row_does_not_take_a_name_another_inserted_since() {
    let facade = crate::testing::open();
    pairing(&facade, "local-a", ACTIVE);
    pairing(&facade, "local-b", ACTIVE);
    add(&facade, &host("erp.example.com", "local-a", HOST_PASSTHROUGH)).unwrap();
    let err = transaction(&facade, true, || {
        save(&facade, "erp.example.com", "local-b", HOST_TERMINATE, None, None)
    })
    .unwrap_err();
    assert_eq!(err.message(), "erp.example.com: another node has this name");
    let held = list(&facade, None).unwrap();
    assert_eq!(held[0].paired_node_id, "local-a");
    assert_eq!(held[0].mode_enum_id, HOST_PASSTHROUGH);
    // the holder's own racing claim changes only the mode
    transaction(&facade, true, || {
        save(&facade, "erp.example.com", "local-a", HOST_TERMINATE, None, None)
    })
    .unwrap();
    assert_eq!(list(&facade, None).unwrap()[0].mode_enum_id, HOST_TERMINATE);
}

/// The pairings' rows with their own queue cap, which 0 or less is refused for and `None` unsets.
#[test]
fn a_pairing_s_queue_cap_is_set_and_unset() {
    let facade = crate::testing::open();
    pairing(&facade, "local-a", ACTIVE);
    set_queue_cap(&facade, "local-a", Some(64)).unwrap();
    assert_eq!(clients(&facade).unwrap()[0].queue_max_megabytes, Some(64));
    assert!(set_queue_cap(&facade, "local-a", Some(0)).is_err());
    assert!(set_queue_cap(&facade, "local-x", Some(5)).is_err());
    set_queue_cap(&facade, "local-a", None).unwrap();
    let rows = clients(&facade).unwrap();
    assert_eq!(rows[0].queue_max_megabytes, None);
    assert_eq!(rows[0].status_id, ACTIVE);
    assert!(!rows[0].linked);
}

/// A host's sticky cookie (plans/Edge-Node2.md §4.4): set with the host, kept when a later save names none (a
/// Configure's host names), cleared by an empty one, and a name that is not a cookie name refused.
#[test]
fn a_host_keeps_its_sticky_cookie_until_one_is_given() {
    let facade = crate::testing::open();
    pairing(&facade, "local-a", ACTIVE);
    let cookie = |name: &str| {
        list(&facade, None)
            .unwrap()
            .into_iter()
            .find(|host| host.host_name == name)
            .and_then(|host| host.sticky_cookie)
    };
    let mut shop = host("shop.example.com", "local-a", HOST_TERMINATE);
    shop.sticky_cookie = Some("JSESSIONID".to_string());
    add(&facade, &shop).unwrap();
    assert_eq!(cookie("shop.example.com").as_deref(), Some("JSESSIONID"));
    add(&facade, &host("shop.example.com", "local-a", HOST_TERMINATE)).unwrap();
    assert_eq!(
        cookie("shop.example.com").as_deref(),
        Some("JSESSIONID"),
        "a save naming no cookie keeps it"
    );
    shop.sticky_cookie = Some(String::new());
    add(&facade, &shop).unwrap();
    assert_eq!(cookie("shop.example.com").as_deref(), Some(""), "cleared: the conf's");
    for bad in ["a b", "a;b", "a=b", "\u{e9}t\u{e9}", &"x".repeat(MAX_COOKIE_NAME + 1)] {
        shop.sticky_cookie = Some(bad.to_string());
        assert!(
            add(&facade, &shop).unwrap_err().message().contains("not a cookie name"),
            "{bad}"
        );
    }
    assert!(valid_cookie_name("moqui.session") && valid_cookie_name("__Host-sid"));
}
