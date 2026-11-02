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
fn calls_are_on_unless_the_setting_turns_them_off() {
    // the only test reading the setting, so setting it here races with nothing
    moqui_lib::env::clear_property(CALLS_ENABLED);
    assert!(calls_enabled(), "on by default, as E7 behaves");
    for off in ["false", "N", "no", "0", " off "] {
        moqui_lib::env::set_property(CALLS_ENABLED, off.to_string());
        assert!(!calls_enabled(), "{off}");
    }
    for on in ["true", "Y", "yes", "1"] {
        moqui_lib::env::set_property(CALLS_ENABLED, on.to_string());
        assert!(calls_enabled(), "{on}");
    }
    moqui_lib::env::clear_property(CALLS_ENABLED);
}

#[test]
fn a_media_address_outside_browsers_cannot_reach_is_named() {
    assert_eq!(media_address_problem("203.0.113.5:10000"), None);
    assert_eq!(media_address_problem("[2001:db8::5]:10000"), None);
    for local in [
        "127.0.0.1:10000",
        "10.1.2.3:10000",
        "192.168.1.5:10000",
        "169.254.1.1:10000",
        "0.0.0.0:10000",
        "[::1]:10000",
        "[fd00::1]:10000",
        "[fe80::1]:10000",
    ] {
        let problem = media_address_problem(local).unwrap_or_default();
        assert!(problem.contains("rtc_public_address"), "{local}: {problem}");
    }
    assert!(media_address_problem("edge.example.net")
        .unwrap()
        .contains("not an address"));
}

#[test]
fn a_link_that_is_down_or_without_media_names_no_media_node() {
    let mut status = LinkStatus::default();
    assert!(linked_media_node(&status).unwrap_err().contains("unpaired"));
    status.state = crate::edge::LinkState::Linked;
    status.peer.instance_id = "edge-1".to_string();
    assert!(linked_media_node(&status).unwrap_err().contains("rtc_media_port"));
    status.peer.media_address = "203.0.113.5:10000".to_string();
    assert_eq!(linked_media_node(&status).unwrap().instance_id, "edge-1");
}
