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

use std::collections::HashMap;
use std::net::SocketAddr;

use super::{base_url_note, cluster_wanted, NO_OTHER_MEMBER};
use crate::listeners::Wanted;
use crate::ListenBind;

fn wanted(bind: &ListenBind, given: &[(&str, &str)], in_cluster: bool) -> Wanted<SocketAddr, ()> {
    let conf: HashMap<String, String> = given
        .iter()
        .map(|(selector, value)| ((*selector).to_string(), (*value).to_string()))
        .collect();
    cluster_wanted(bind, &|selector: &str| conf.get(selector).cloned(), in_cluster)
}

fn on_at(address: &str) -> Wanted<SocketAddr, ()> {
    Wanted::On {
        bind: address.parse().unwrap(),
        in_place: (),
    }
}

/// `auto` opens the port only while this member is in a cluster, `on` always and `off` never; a node reached in
/// process only has none whatever the conf says (plans/Runtime-Config.md §3.5).
#[test]
fn the_cluster_port_is_wanted_by_its_mode_and_the_cluster() {
    let bind = ListenBind::Property;
    let auto = [("cluster-member@listen", "auto"), ("cluster-member@https-port", "8444")];
    assert_eq!(wanted(&bind, &auto, false), Wanted::Off(NO_OTHER_MEMBER.to_string()));
    assert_eq!(wanted(&bind, &auto, true), on_at("0.0.0.0:8444"));
    // left out of the conf is the default, auto
    assert_eq!(wanted(&bind, &[], false), Wanted::Off(NO_OTHER_MEMBER.to_string()));
    assert_eq!(wanted(&bind, &[], true), on_at("0.0.0.0:8444"));
    let always = [("cluster-member@listen", " on "), ("cluster-member@https-port", "0")];
    assert_eq!(wanted(&bind, &always, false), on_at("0.0.0.0:0"));
    let never = [("cluster-member@listen", "off")];
    for in_cluster in [false, true] {
        assert_eq!(
            wanted(&bind, &never, in_cluster),
            Wanted::Off("cluster-member@listen is off".to_string())
        );
    }
    let address = SocketAddr::from(([127, 0, 0, 1], 0));
    for bind in [ListenBind::Property, ListenBind::Address(address)] {
        assert_eq!(wanted(&bind, &always, false), on_at("0.0.0.0:0"));
    }
    for listen in ["on", "auto", "off"] {
        assert_eq!(
            wanted(&ListenBind::None, &[("cluster-member@listen", listen)], true),
            Wanted::Off("this node is reached in process only".to_string())
        );
    }
}

/// Under `auto` a member that peers reach elsewhere opens no cluster port, in a cluster or not: a base URL that is not
/// `https://`, or with none, the `notify` or `ssh` transport; `on` opens it all the same.
#[test]
fn a_member_reached_elsewhere_opens_no_cluster_port_under_auto() {
    let bind = ListenBind::Property;
    for elsewhere in [
        [("cluster-member@transport", "notify"), ("cluster-member@base-url", "")],
        [("cluster-member@transport", "ssh"), ("cluster-member@base-url", "")],
        [
            ("cluster-member@transport", ""),
            ("cluster-member@base-url", "pg-notify:"),
        ],
        [
            ("cluster-member@transport", "ssh"),
            ("cluster-member@base-url", "ssh://10.0.0.1:2222"),
        ],
    ] {
        let auto: Vec<(&str, &str)> = elsewhere
            .iter()
            .copied()
            .chain([("cluster-member@listen", "auto")])
            .collect();
        let Wanted::Off(reason) = wanted(&bind, &auto, true) else {
            panic!("{elsewhere:?} opens no port under auto");
        };
        assert!(reason.contains("cluster-member@"), "{reason}");
        let on: Vec<(&str, &str)> = elsewhere
            .iter()
            .copied()
            .chain([("cluster-member@listen", "on")])
            .collect();
        assert_eq!(
            wanted(&bind, &on, false),
            on_at("0.0.0.0:8444"),
            "{elsewhere:?} under on"
        );
    }
    let https = [
        ("cluster-member@listen", "auto"),
        ("cluster-member@base-url", "https://10.0.0.1:8444"),
    ];
    assert_eq!(wanted(&bind, &https, true), on_at("0.0.0.0:8444"));
}

/// An explicit base URL that names another port than the bound one is said on the row, since peers post there and it
/// waits for a restart; the default, or one that names the bound port or none, says nothing.
#[test]
fn a_base_url_naming_another_port_is_said_on_the_row() {
    let note = |base: &'static str, bound: u16| {
        base_url_note(
            &move |selector: &str| {
                if selector == "cluster-member@base-url" {
                    base.to_string()
                } else {
                    String::new()
                }
            },
            bound,
        )
    };
    let said = note("https://10.0.0.1:8444", 9444).expect("another port");
    assert!(said.contains("names port 8444, not 9444"), "{said}");
    assert_eq!(note("https://10.0.0.1:9444", 9444), None);
    assert_eq!(note("", 9444), None, "the default names the bound port once rewritten");
    assert_eq!(note("https://members.example", 9444), None, "no port named");
    assert_eq!(note("pg-notify:", 9444), None);
}

/// The port and address come from the same reading; one that is not a port or an address leaves the port off, named on
/// the row, and a mode that is none of the XSD's three (in another case, or empty) is named too.
#[test]
fn the_cluster_ports_address_and_a_bad_setting_are_named() {
    let bind = ListenBind::Property;
    assert_eq!(
        wanted(
            &bind,
            &[
                ("cluster-member@listen", "on"),
                ("cluster-member@https-port", " 9444 "),
                ("cluster-member@bind-address", "127.0.0.1"),
            ],
            false
        ),
        on_at("127.0.0.1:9444")
    );
    let Wanted::Off(port) = wanted(
        &bind,
        &[("cluster-member@listen", "on"), ("cluster-member@https-port", "nope")],
        false,
    ) else {
        panic!("a port that is not one is no bind");
    };
    assert!(port.contains("cluster-member@https-port nope"), "{port}");
    let Wanted::Off(address) = wanted(
        &bind,
        &[("cluster-member@listen", "on"), ("cluster-member@bind-address", "here")],
        false,
    ) else {
        panic!("an address that is not one is no bind");
    };
    assert!(address.contains("cluster-member@bind-address here"), "{address}");
    for refused in ["yes", "ON", ""] {
        assert_eq!(
            wanted(&bind, &[("cluster-member@listen", refused)], true),
            Wanted::Off(format!("cluster-member@listen \"{refused}\" is not auto, on or off"))
        );
    }
}
