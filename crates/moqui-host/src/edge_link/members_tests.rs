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

const PAIRED: &str = "https://edge.example.test";

fn member(id: &str, url: &str) -> EdgeMember {
    EdgeMember {
        instance_id: id.to_string(),
        link_url: url.to_string(),
    }
}

fn link(member: Option<&str>, url: &str, linked: bool, failures: u32) -> LinkView {
    LinkView {
        member: member.map(str::to_string),
        url: url.to_string(),
        linked,
        failures,
    }
}

#[test]
fn a_list_is_checked_as_input() {
    assert!(check_list(&[member("edge-1", "https://e1.test:8443"), member("edge-2", "")]).is_ok());
    assert!(check_list(&[member("edge-1", ""), member("edge-1", "")]).is_err_and(|why| why.contains("twice")));
    assert!(check_list(&[member("bad id", "")]).is_err());
    assert!(check_list(&[member("edge@1", "")]).is_err());
    assert!(check_list(&[member("edge-1", "http://e1.test")]).is_err_and(|why| why.contains("not https")));
    assert!(check_list(&[member("edge-1", "https://")]).is_err());
    assert!(check_list(&[member("edge-1", &format!("https://{}.test", "a".repeat(600)))]).is_err());
    let long: Vec<EdgeMember> = (0..=MAX_EDGE_MEMBERS).map(|at| member(&format!("e{at}"), "")).collect();
    assert!(check_list(&long).is_err_and(|why| why.contains("more than")));
}

#[test]
fn with_no_list_the_paired_url_is_dialed() {
    let plan = plan(&[], PAIRED, &[]);
    assert!(plan.paired_url);
    assert!(plan.start.is_empty() && plan.retire.is_empty());
    // an edge node before E18 says nothing: the link it made is kept where it dials
    let plan = super::plan(&[], PAIRED, &[link(Some("edge-1"), PAIRED, false, 5)]);
    assert!(!plan.paired_url, "that link dials the paired URL itself");
    assert!(plan.retire.is_empty() && plan.dial_at.is_empty());
}

#[test]
fn every_member_with_an_address_gets_a_link_and_the_paired_link_becomes_its_members() {
    let list = [member("edge-1", "https://e1.test"), member("edge-2", "https://e2.test")];
    // the first link, to the paired URL, came up as edge-1: it dials edge-1's own address from now, and edge-2 starts
    let plan = plan(&list, PAIRED, &[link(Some("edge-1"), PAIRED, true, 0)]);
    assert_eq!(
        plan.dial_at,
        vec![("edge-1".to_string(), "https://e1.test".to_string())]
    );
    assert_eq!(plan.start, vec![("edge-2".to_string(), "https://e2.test".to_string())]);
    assert!(plan.retire.is_empty());
    assert!(!plan.paired_url);
}

#[test]
fn a_member_gone_from_the_list_is_retired_and_a_new_one_started() {
    let list = [member("edge-1", "https://e1.test"), member("edge-3", "https://e3.test")];
    let links = [
        link(Some("edge-1"), "https://e1.test", true, 0),
        link(Some("edge-2"), "https://e2.test", false, 3),
    ];
    let plan = plan(&list, PAIRED, &links);
    assert_eq!(plan.retire, vec!["edge-2".to_string()]);
    assert_eq!(plan.start, vec![("edge-3".to_string(), "https://e3.test".to_string())]);
    assert!(plan.dial_at.is_empty());
}

#[test]
fn a_member_with_no_address_is_reached_at_the_paired_url() {
    // a single edge node with no edge_link_url
    let list = [member("edge-1", "")];
    let plan = plan(&list, PAIRED, &[link(Some("edge-1"), PAIRED, false, 9)]);
    assert!(plan.start.is_empty() && plan.dial_at.is_empty() && plan.retire.is_empty());
    assert!(!plan.paired_url, "its own link dials it");
    // a restart with that list kept and no link yet
    assert!(super::plan(&list, PAIRED, &[]).paired_url);
}

#[test]
fn the_paired_url_is_dialed_again_only_when_every_member_fails() {
    let list = [member("edge-1", "https://e1.test"), member("edge-2", "https://e2.test")];
    // a restart with the list kept: each member at its own address, not the paired URL
    let fresh = plan(&list, PAIRED, &[]);
    assert_eq!(fresh.start.len(), 2);
    assert!(!fresh.paired_url);
    let failing = |failures| {
        [
            link(Some("edge-1"), "https://e1.test", false, failures),
            link(Some("edge-2"), "https://e2.test", false, failures),
        ]
    };
    assert!(!plan(&list, PAIRED, &failing(PAIRED_URL_AFTER - 1)).paired_url);
    assert!(plan(&list, PAIRED, &failing(PAIRED_URL_AFTER)).paired_url);
    let mut one_up = failing(PAIRED_URL_AFTER);
    one_up[1].linked = true;
    assert!(!plan(&list, PAIRED, &one_up).paired_url, "a member is linked");
    // a list that names the paired URL as another member's address, which reaches the first: dialed too once it fails
    let misdirected = [member("edge-2", PAIRED)];
    assert!(!plan(&misdirected, PAIRED, &[link(Some("edge-2"), PAIRED, false, 1)]).paired_url);
    assert!(
        plan(
            &misdirected,
            PAIRED,
            &[link(Some("edge-2"), PAIRED, false, PAIRED_URL_AFTER)]
        )
        .paired_url
    );
}

#[test]
fn the_kept_list_is_for_one_pairing_and_comes_back() {
    let dir = std::env::temp_dir().join(format!("moqui-edge-members-{}", std::process::id()));
    let path = file(&dir);
    let mut known = Known::new(PAIRED, "ab");
    assert!(known.set_members(vec![member("edge-1", "https://e1.test")]));
    assert!(
        !known.set_members(vec![member("edge-1", "https://e1.test")]),
        "unchanged"
    );
    assert!(known.learn("edge-1"));
    assert!(!known.learn("edge-1"));
    known.save(Some(&path));
    let loaded = Known::load(Some(&path), PAIRED, "ab");
    assert_eq!(loaded, known);
    assert_eq!(loaded.url_of("edge-1", PAIRED), "https://e1.test");
    assert_eq!(loaded.url_of("edge-9", PAIRED), PAIRED);
    // another edge node's (paired again): not used
    assert!(Known::load(Some(&path), PAIRED, "cd").members.is_empty());
    assert!(Known::load(Some(&path), "https://other.test", "ab").ids.is_empty());
    // not a list: nothing, and no error out
    std::fs::write(&path, "not json").unwrap();
    assert!(Known::load(Some(&path), PAIRED, "ab").members.is_empty());
    // a list that fails the checks is dropped whole
    std::fs::write(
        &path,
        r#"{"url":"https://edge.example.test","pin":"ab","members":[{"instanceId":"a b","linkUrl":""}],"ids":["edge-1"]}"#,
    )
    .unwrap();
    let checked = Known::load(Some(&path), PAIRED, "ab");
    assert!(checked.members.is_empty());
    assert_eq!(checked.ids, vec!["edge-1".to_string()]);
    forget(&dir);
    assert!(!path.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_edge_member_lists_itself_and_its_live_peers_with_their_addresses() {
    let row = |id: &str, live: bool, url: &str| moqui_core::cluster::MemberReport {
        instance_id: id.to_string(),
        heartbeat_age_ms: if live { 1_000 } else { 600_000 },
        live,
        version: String::new(),
        edge_link_url: url.to_string(),
    };
    let rows = [
        row("edge-1", true, "https://stale.test"),
        row("edge-2", true, "https://e2.test:8443"),
        row("edge-3", false, "https://e3.test"),
        row("edge-4", true, "http://e4.test"),
        row("bad id", true, "https://e5.test"),
    ];
    let list = of_cluster("edge-1", "https://e1.test", &rows);
    assert_eq!(
        list,
        vec![
            member("edge-1", "https://e1.test"),
            member("edge-2", "https://e2.test:8443"),
            member("edge-4", ""),
        ],
        "in id order, itself with its own address; the stopped member and the bad id left out; a bad address as none"
    );
    assert!(check_list(&list).is_ok(), "what a local node takes");
    // a single edge node with no row (no cluster) or no address
    assert_eq!(of_cluster("edge-1", "", &[]), vec![member("edge-1", "")]);
}

#[test]
fn the_same_members_in_another_order_are_no_change() {
    let mut known = Known::new(PAIRED, "ab");
    assert!(known.set_members(vec![
        member("edge-2", "https://e2.test"),
        member("edge-1", "https://e1.test")
    ]));
    assert!(
        !known.set_members(vec![
            member("edge-1", "https://e1.test"),
            member("edge-2", "https://e2.test")
        ]),
        "each member answers in its own order: no change, no write, no reconcile"
    );
    assert!(known.set_members(vec![member("edge-1", "https://e1.test")]));
}

#[test]
fn a_member_whose_own_row_is_stale_does_not_list_itself() {
    let row = |id: &str, live: bool| moqui_core::cluster::MemberReport {
        instance_id: id.to_string(),
        heartbeat_age_ms: if live { 1_000 } else { 600_000 },
        live,
        version: String::new(),
        edge_link_url: format!("https://{id}.test"),
    };
    let rows = [row("edge-1", true), row("edge-2", false)];
    // the stale member answers as its live peer does, so their lists never flap the local node's links
    let from_stale = of_cluster("edge-2", "https://edge-2.test", &rows);
    let from_live = of_cluster("edge-1", "https://edge-1.test", &rows);
    assert_eq!(from_stale, from_live);
    assert_eq!(from_live, vec![member("edge-1", "https://edge-1.test")]);
}
