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
use crate::edge::media::SipTrunkState;

fn edge() -> EdgeInfo {
    EdgeInfo {
        instance_id: "edge-1".to_string(),
        sip_address: "203.0.113.5:5060".to_string(),
        sip_tls_address: "203.0.113.5:5061".to_string(),
        ..EdgeInfo::default()
    }
}

fn live_row(now: i64) -> SipRow {
    SipRow {
        sip_address: "203.0.113.5:5060".to_string(),
        tls_address: "203.0.113.5:5061".to_string(),
        sip_domain: "sip.example.com".to_string(),
        registrar: true,
        heartbeat_ms: now - 5_000,
        edge_linked: true,
    }
}

fn trunk(id: &str, state: &str, error: &str) -> SipTrunkState {
    SipTrunkState {
        voice_trunk_id: id.to_string(),
        name: format!("{id} name"),
        state: state.to_string(),
        error: error.to_string(),
    }
}

/// The only test reading the setting, so setting it here races with nothing: the setting, the sets it holds back,
/// and the row check's two answers.
#[test]
fn phones_are_on_unless_the_setting_turns_them_off_and_off_holds_back_the_trunks_and_fallbacks() {
    moqui_lib::env::clear_property(SIP_ENABLED);
    assert!(sip_enabled(), "on by default, as E8 behaves");
    assert!(!withheld(TRUNKS_SET) && !withheld(FALLBACKS_SET));
    let now = 1_000_000;
    let (ok, text) = row_check(Some(&live_row(now)), &edge(), now);
    assert!(ok && text.contains("203.0.113.5:5060, 203.0.113.5:5061"), "{text}");
    for off in ["false", "N", "no", "0", " off "] {
        moqui_lib::env::set_property(SIP_ENABLED, off.to_string());
        assert!(!sip_enabled(), "{off}");
    }
    assert!(
        withheld(TRUNKS_SET) && withheld(FALLBACKS_SET),
        "off: both sets go empty"
    );
    assert!(!withheld("mailRecipients"), "only the phones' sets");
    let (ok, text) = row_check(Some(&live_row(now)), &edge(), now);
    assert!(ok && text.contains("takes no new call"), "{text}");
    for on in ["true", "Y", "yes", "1"] {
        moqui_lib::env::set_property(SIP_ENABLED, on.to_string());
        assert!(sip_enabled(), "{on}");
    }
    moqui_lib::env::clear_property(SIP_ENABLED);
}

#[test]
fn the_row_check_names_a_missing_unlinked_stale_or_changed_row() {
    let now = 1_000_000;
    assert!(row_check(None, &edge(), now).1.contains("no SIP node row"));
    let unlinked = SipRow {
        edge_linked: false,
        ..live_row(now)
    };
    assert!(row_check(Some(&unlinked), &edge(), now)
        .1
        .contains("not kept by the link"));
    let stale = SipRow {
        heartbeat_ms: now - STALE_MS - 1,
        ..live_row(now)
    };
    assert!(row_check(Some(&stale), &edge(), now).1.contains("stale"));
    let moved = SipRow {
        sip_address: "203.0.113.9:5060".to_string(),
        ..live_row(now)
    };
    let (ok, text) = row_check(Some(&moved), &edge(), now);
    assert!(!ok && text.contains("Hello's addresses"), "{text}");
}

#[test]
fn the_trunk_check_wants_every_enabled_trunk_loaded_and_none_failed() {
    let local = vec![
        ("T1".to_string(), "Main".to_string()),
        ("T2".to_string(), "Backup".to_string()),
    ];
    let mut offer = SipOffer {
        available: true,
        trunks: vec![trunk("T1", "registered", "")],
        ..SipOffer::default()
    };
    assert_eq!(trunk_check(&[], &offer).0, None, "no trunk here: not checked");
    let (ok, text) = trunk_check(&local, &offer);
    assert!(ok == Some(false) && text.contains("not loaded Backup"), "{text}");
    offer.trunks.push(trunk("T2", "failed", "403 Forbidden"));
    let (ok, text) = trunk_check(&local, &offer);
    assert!(ok == Some(false) && text.contains("T2 name (403 Forbidden)"), "{text}");
    offer.trunks[1].state = "loaded".to_string();
    let (ok, text) = trunk_check(&local, &offer);
    assert!(
        ok == Some(true) && text.contains("T1 name (registered), T2 name (loaded)"),
        "{text}"
    );
    offer.other_nodes = true;
    let (ok, text) = trunk_check(&local, &offer);
    assert!(ok == Some(false) && text.contains("Other local nodes"), "{text}");
}

#[test]
fn the_fallback_check_wants_the_copy_complete_and_nothing_waiting() {
    let offer = SipOffer {
        available: true,
        fallbacks: 2,
        ..SipOffer::default()
    };
    let set = SetStatus {
        name: FALLBACKS_SET.to_string(),
        epoch: "e".to_string(),
        acked: 3,
        ..SetStatus::default()
    };
    assert!(fallback_check(2, &offer, None).1.contains("does not replicate"));
    let (ok, text) = fallback_check(2, &offer, Some(&set));
    assert!(ok && text.contains("each of the 2 numbers"), "{text}");
    let (ok, text) = fallback_check(3, &offer, Some(&set));
    assert!(!ok && text.contains("holds 2") && text.contains("has 3"), "{text}");
    let waiting = SetStatus {
        waiting: 1,
        ..set.clone()
    };
    let (ok, text) = fallback_check(2, &offer, Some(&waiting));
    assert!(!ok && text.contains("1 changes waiting"), "{text}");
    let none = SipOffer {
        fallbacks: 0,
        ..offer.clone()
    };
    let (ok, text) = fallback_check(0, &none, Some(&set));
    assert!(ok && text.contains("refused (480)"), "{text}");
    let others = SipOffer {
        other_nodes: true,
        ..offer
    };
    assert!(!fallback_check(2, &others, Some(&set)).0);
}

#[test]
fn a_link_that_is_down_or_without_sip_names_no_sip_node() {
    let mut status = crate::edge::LinkStatus::default();
    assert!(linked_sip_node(&status).unwrap_err().contains("unpaired"));
    status.state = crate::edge::LinkState::Linked;
    status.peer.instance_id = "edge-1".to_string();
    assert!(linked_sip_node(&status).unwrap_err().contains("sip_port"));
    status.peer.sip_tls_address = "203.0.113.5:5061".to_string();
    assert_eq!(linked_sip_node(&status).unwrap().instance_id, "edge-1");
}
