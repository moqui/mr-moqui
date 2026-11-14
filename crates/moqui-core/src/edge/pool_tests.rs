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

fn member(id: &str, live: bool, version: &str) -> ClusterMemberInfo {
    ClusterMemberInfo {
        instance_id: id.to_string(),
        heartbeat_age_ms: if live { 1_000 } else { 600_000 },
        live,
        version: version.to_string(),
    }
}

fn linked(id: &str, version: &str) -> LinkedMember {
    LinkedMember {
        instance_id: id.to_string(),
        generation: 1,
        down: true,
        binary_version: version.to_string(),
        ..LinkedMember::default()
    }
}

#[test]
fn a_member_state_reads_back_and_only_a_live_member_takes_work() {
    for state in [
        MemberState::Live,
        MemberState::Starting,
        MemberState::Draining,
        MemberState::Stopping,
    ] {
        assert_eq!(MemberState::parse(state.as_str()), Some(state));
        assert_eq!(state.takes_work(), state == MemberState::Live);
    }
    assert_eq!(MemberState::parse(""), Some(MemberState::Live), "a member before E17");
    assert_eq!(MemberState::parse("asleep"), None);
}

#[test]
fn a_member_list_is_checked_as_input() {
    let good = vec![member("a", true, "0.1.0"), member("b", false, "0.1.0")];
    assert!(check_member_list(&good).is_ok());
    let too_many: Vec<_> = (0..=MAX_CLUSTER_MEMBERS)
        .map(|n| member(&format!("m{n}"), true, ""))
        .collect();
    assert!(check_member_list(&too_many).unwrap_err().contains("more than"));
    for bad in ["", "a b", "x@pairing", "line\nbreak", &"x".repeat(41)] {
        assert!(check_member_list(&[member(bad, true, "")]).is_err(), "{bad:?}");
    }
    let twice = vec![member("a", true, ""), member("a", false, "")];
    assert!(check_member_list(&twice).unwrap_err().contains("twice"));
    assert!(check_member_list(&[member("a", true, &"9".repeat(41))]).is_err());
    assert!(check_member_list(&[member("a", true, "1\u{0}")]).is_err());
}

#[test]
fn admission_asks_the_linked_members_list_and_with_none_linked_the_key_admits() {
    let linked_a = vec!["a".to_string()];
    let list = vec![member("a", true, ""), member("b", true, ""), member("c", false, "")];
    let facts = |id| AdmissionFacts {
        member: id,
        linked: &linked_a,
        listed: Some(&list),
        last_member: "a",
        ..AdmissionFacts::default()
    };
    // a reconnect of a linked member; a live member of its list; a stale one and a stranger refused
    assert_eq!(admission(&facts("a")), Admission::Admit);
    assert_eq!(admission(&facts("b")), Admission::Admit);
    assert_eq!(admission(&facts("c")), Admission::Refuse(JOIN_FIRST.to_string()));
    assert_eq!(admission(&facts("z")), Admission::Refuse(JOIN_FIRST.to_string()));
    // no answer from the linked member: refused, as a list without it
    let unasked = AdmissionFacts {
        listed: None,
        ..facts("b")
    };
    assert_eq!(admission(&unasked), Admission::Refuse(JOIN_FIRST.to_string()));

    // none linked: the key admits; the last member and a member of the kept list come back, a new node takes over
    let none = |id, known: &'static [ClusterMemberInfo]| AdmissionFacts {
        member: id,
        known,
        last_member: "a",
        ..AdmissionFacts::default()
    };
    assert_eq!(admission(&none("a", &[])), Admission::Admit);
    static KNOWN: std::sync::LazyLock<Vec<ClusterMemberInfo>> =
        std::sync::LazyLock::new(|| vec![member("a", true, ""), member("b", true, "")]);
    assert_eq!(admission(&none("b", &KNOWN)), Admission::Admit);
    assert_eq!(
        admission(&none("standby", &[])),
        Admission::TakeOver { from: "a".to_string() }
    );
    // a pairing that has never linked admits its first node
    let first = AdmissionFacts {
        member: "anything",
        ..AdmissionFacts::default()
    };
    assert_eq!(admission(&first), Admission::Admit);

    // held for approval until it is the one approved
    let held = AdmissionFacts {
        hold: true,
        ..none("standby", &[])
    };
    assert_eq!(admission(&held), Admission::Hold { from: "a".to_string() });
    let approved = AdmissionFacts {
        approved: "standby",
        ..held
    };
    assert_eq!(admission(&approved), Admission::TakeOver { from: "a".to_string() });
    // the hold is for a new node only: the last member comes back
    assert_eq!(
        admission(&AdmissionFacts {
            hold: true,
            ..none("a", &[])
        }),
        Admission::Admit
    );
}

#[test]
fn the_report_shows_each_member_linked_listed_and_refused() {
    let links = vec![linked("a", "0.2.0"), linked("d", "0.2.0"), linked("e", "0.1.0")];
    let list = vec![
        member("a", true, "0.2.0"),
        member("b", true, "0.2.0"),
        member("c", false, "0.2.0"),
        member("e", true, "0.1.0"),
    ];
    let refused = vec![
        RefusedMember {
            instance_id: "b".to_string(),
            message: JOIN_FIRST.to_string(),
            at: 5,
            ..RefusedMember::default()
        },
        RefusedMember {
            instance_id: "old".to_string(),
            message: JOIN_FIRST.to_string(),
            at: 6,
            ..RefusedMember::default()
        },
        RefusedMember {
            instance_id: "standby".to_string(),
            message: HELD_FOR_APPROVAL.to_string(),
            at: 7,
            held: true,
            ..RefusedMember::default()
        },
    ];
    let report = report("pair", &links, Some(&list), &refused);
    let condition = |id: &str| {
        report
            .iter()
            .find(|member| member.instance_id == id)
            .map(|member| member.condition.as_str())
    };
    assert_eq!(condition("a"), Some("ok"));
    assert_eq!(
        condition("b"),
        Some("notLinked"),
        "live in its cluster, not linked: a fault"
    );
    assert_eq!(condition("c"), Some("stopped"));
    assert_eq!(
        condition("d"),
        Some("staleHeartbeat"),
        "linked, and not in its cluster's list"
    );
    assert_eq!(condition("e"), Some("otherVersion"));
    assert_eq!(condition("old"), Some("refused"));
    assert_eq!(condition("standby"), Some("held"));
    let b = report.iter().find(|member| member.instance_id == "b").unwrap();
    assert_eq!(b.message, JOIN_FIRST, "why a live member is not linked");
    assert!(report.iter().all(|member| member.paired_node_id == "pair"));
    // with no list yet, a linked member is not stale
    let unlisted = super::report("pair", &links[..1], None, &[]);
    assert_eq!(unlisted[0].condition, "ok");
}

#[test]
fn a_member_id_is_its_own_only_with_its_pinned_key() {
    let linked = vec!["a".to_string()];
    let list = vec![member("a", true, ""), member("b", true, "")];
    // a stolen .env naming a linked member's id, or a listed one's, with another key: refused while one is linked
    for id in ["a", "b"] {
        let facts = AdmissionFacts {
            member: id,
            linked: &linked,
            listed: Some(&list),
            last_member: "a",
            pinned: Some(false),
            ..AdmissionFacts::default()
        };
        assert_eq!(admission(&facts), Admission::Refuse(JOIN_FIRST.to_string()), "{id}");
    }
    // with none linked, the paired member's id with another key is a new node: a takeover, held when the pairing holds
    let none = AdmissionFacts {
        member: "a",
        last_member: "a",
        pinned: Some(false),
        ..AdmissionFacts::default()
    };
    assert_eq!(admission(&none), Admission::TakeOver { from: "a".to_string() });
    assert_eq!(
        admission(&AdmissionFacts { hold: true, ..none }),
        Admission::Hold { from: "a".to_string() }
    );
    // a pinned member comes back with none linked even when it was not the last and the kept list is gone (a restart)
    let back = AdmissionFacts {
        member: "b",
        last_member: "a",
        pinned: Some(true),
        hold: true,
        ..AdmissionFacts::default()
    };
    assert_eq!(admission(&back), Admission::Admit);
    // a kept list's member that is not live there is not taken as returning
    let stale = vec![member("c", false, "")];
    let facts = AdmissionFacts {
        member: "c",
        known: &stale,
        last_member: "a",
        ..AdmissionFacts::default()
    };
    assert_eq!(admission(&facts), Admission::TakeOver { from: "a".to_string() });
}

#[test]
fn the_node_a_takeover_displaced_is_refused_until_cleared() {
    let facts = AdmissionFacts {
        member: "first",
        last_member: "standby",
        pinned: Some(true),
        displaced: "first",
        ..AdmissionFacts::default()
    };
    assert_eq!(
        admission(&facts),
        Admission::Refuse(DISPLACED.to_string()),
        "with none linked too"
    );
    assert_eq!(admission(&AdmissionFacts { displaced: "", ..facts }), Admission::Admit);
}

#[test]
fn member_keys_are_pinned_newest_last_and_capped() {
    let mut json = String::from("[]");
    for n in 0..=MAX_PINNED {
        json = pin_key(&pinned_keys(&json), &format!("m{n}"), &format!("k{n}"));
    }
    let pins = pinned_keys(&json);
    assert_eq!(pins.len(), MAX_PINNED);
    assert_eq!(key_matches(&pins, "m0", "k0"), None, "the oldest went");
    assert_eq!(key_matches(&pins, "m1", "k1"), Some(true));
    assert_eq!(key_matches(&pins, "m1", "other"), Some(false));
    let repinned = pinned_keys(&pin_key(&pins, "m1", "k1b"));
    assert_eq!(repinned.last().unwrap(), &("m1".to_string(), "k1b".to_string()));
    assert_eq!(key_matches(&pinned_keys(&unpin_key(&pins, "m1")), "m1", "k1"), None);
    assert!(pinned_keys("not json").is_empty());
}

#[test]
fn a_forgotten_key_makes_its_id_a_new_node() {
    let pins = pinned_keys(&pin_key(&[], "a", "ka"));
    let forgot = pinned_keys(&forget_key(&pins, "a"));
    assert!(forgotten(&forgot, "a"));
    assert_eq!(key_matches(&forgot, "a", "ka"), None, "no key is pinned for it now");
    // with none linked, the last member's id with its key forgotten takes over (or is held), not comes back
    let facts = AdmissionFacts {
        member: "a",
        last_member: "a",
        forgotten: true,
        ..AdmissionFacts::default()
    };
    assert_eq!(admission(&facts), Admission::TakeOver { from: "a".to_string() });
    assert_eq!(
        admission(&AdmissionFacts { hold: true, ..facts }),
        Admission::Hold { from: "a".to_string() }
    );
    // its next link pins the key it brings, and it is a member again
    let repinned = pinned_keys(&pin_key(&forgot, "a", "ka2"));
    assert!(!forgotten(&repinned, "a"));
    assert_eq!(key_matches(&repinned, "a", "ka2"), Some(true));
}

#[test]
fn a_key_is_claimed_once_and_past_the_cap_the_stable_members_stay() {
    let pins = pinned_keys(&pin_key(&[], "a", "ka"));
    // none pinned: pinned; the same: kept, moved to newest; another: refused, unless a takeover was decided
    assert!(matches!(claim_key(&pins, "b", "kb", false, &[]), KeyClaim::Pin(_)));
    assert!(matches!(claim_key(&pins, "a", "ka", false, &[]), KeyClaim::Pin(_)));
    assert_eq!(claim_key(&pins, "a", "other", false, &[]), KeyClaim::Refuse);
    let KeyClaim::Pin(json) = claim_key(&pins, "a", "other", true, &[]) else {
        panic!("a takeover pins its key")
    };
    assert_eq!(key_matches(&pinned_keys(&json), "a", "other"), Some(true));
    // a forgotten id takes the key it brings
    let forgot = pinned_keys(&forget_key(&pins, "a"));
    assert!(matches!(claim_key(&forgot, "a", "new", false, &[]), KeyClaim::Pin(_)));

    // churn past the cap: a forgotten marker goes first, then the oldest not kept; the kept (linked, last, listed) stay
    let mut json = forget_key(&pins, "gone");
    json = pin_key(&pinned_keys(&json), "stable", "ks");
    for n in 0..MAX_PINNED {
        let keep = vec!["stable".to_string(), "a".to_string()];
        let KeyClaim::Pin(next) = claim_key(&pinned_keys(&json), &format!("churn{n}"), "k", false, &keep) else {
            panic!("pinned")
        };
        json = next;
    }
    let pins = pinned_keys(&json);
    assert_eq!(pins.len(), MAX_PINNED);
    assert!(!forgotten(&pins, "gone"), "the forgotten marker went first");
    assert_eq!(key_matches(&pins, "stable", "ks"), Some(true), "a kept member stayed");
    assert_eq!(key_matches(&pins, "a", "ka"), Some(true), "the last member stayed");
    assert_eq!(key_matches(&pins, "churn0", "k"), None, "the oldest churn went");
}

#[test]
fn the_last_member_with_a_new_key_is_a_takeover_from_itself() {
    // its key forgotten (or its key file made again): with none linked a takeover from itself, held when set
    let facts = AdmissionFacts {
        member: "a",
        last_member: "a",
        forgotten: true,
        ..AdmissionFacts::default()
    };
    assert_eq!(admission(&facts), Admission::TakeOver { from: "a".to_string() });
    assert_eq!(
        admission(&AdmissionFacts { hold: true, ..facts }),
        Admission::Hold { from: "a".to_string() }
    );
}
