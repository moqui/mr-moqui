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

use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

use super::*;
use crate::edge::replica::applied_count;

fn open() -> (crate::context::ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    (ctx, facade)
}

/// Another member's notice counts each set it names as applied here and asks for the host table; one with no sender
/// asks nothing.
#[test]
fn another_members_notice_moves_the_sets_and_asks_for_the_hosts() {
    let (_ctx, facade) = open();
    let before = applied_count(&facade, "mailRecipients");
    let taken = take(
        &facade,
        &serde_json::json!({"from": "edge-2", "hosts": true, "sets": ["mailRecipients"]}),
    );
    assert_eq!(
        taken,
        Taken {
            hosts: true,
            sets: vec!["mailRecipients".to_string()],
            links: Vec::new(),
        }
    );
    assert_eq!(applied_count(&facade, "mailRecipients"), before + 1);
    assert_eq!(applied_count(&facade, "voiceTrunks"), 0, "a set not named stays");

    let none = take(&facade, &serde_json::json!({"hosts": true, "sets": ["mailRecipients"]}));
    assert_eq!(none, Taken::default());
    assert_eq!(applied_count(&facade, "mailRecipients"), before + 1);
}

/// A member that may have missed notices counts every set as changed and loads its host table again.
#[test]
fn recovery_moves_every_set_and_loads_the_hosts() {
    let (ctx, facade) = open();
    let loads = Arc::new(AtomicUsize::new(0));
    let counting = Arc::clone(&loads);
    install(
        ctx.factory(),
        Arc::new(move || {
            counting.fetch_add(1, Ordering::AcqRel);
        }),
        Arc::new(|_: &[LinkOp]| {}),
    );
    let trunks = applied_count(&facade, "voiceTrunks");
    let recipients = applied_count(&facade, "mailRecipients");
    recovered(ctx.factory(), &facade);
    assert_eq!(applied_count(&facade, "voiceTrunks"), trunks + 1);
    assert_eq!(applied_count(&facade, "mailRecipients"), recipients + 1);
    let started = Instant::now();
    while loads.load(Ordering::Acquire) == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the host table loaded again"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A burst of requests while a load runs loads at most once more after it.
#[test]
fn a_burst_of_notices_loads_the_hosts_once_more_at_most() {
    let loads = Arc::new(AtomicUsize::new(0));
    let counting = Arc::clone(&loads);
    let reloader = Arc::new(Reloader {
        reload: Arc::new(move || {
            counting.fetch_add(1, Ordering::AcqRel);
            std::thread::sleep(Duration::from_millis(100));
        }),
        pending: AtomicBool::new(false),
        running: AtomicBool::new(false),
    });
    for _ in 0..20 {
        reloader.request();
    }
    let started = Instant::now();
    while reloader.running.load(Ordering::Acquire) || reloader.pending.load(Ordering::Acquire) {
        assert!(started.elapsed() < Duration::from_secs(5), "the loads end");
        std::thread::sleep(Duration::from_millis(10));
    }
    let count = loads.load(Ordering::Acquire);
    assert!((1..=2).contains(&count), "{count} loads");
}

/// What another member did to a pairing's links goes in the notice, and one that does not read is left out.
#[test]
fn link_operations_read_back_and_bad_ones_are_left_out() {
    let (_ctx, facade) = open();
    let ops = vec![
        LinkOp::Revoked {
            node: "local-a".to_string(),
        },
        LinkOp::Paired {
            node: "local-b".to_string(),
            key_changed: false,
        },
        LinkOp::MemberForgotten {
            node: "local-a".to_string(),
            member: "m-2".to_string(),
        },
    ];
    let mut links: Vec<serde_json::Value> = ops.iter().map(LinkOp::to_json).collect();
    links.push(serde_json::json!({"op": "revoked"}));
    links.push(serde_json::json!({"op": "other", "node": "local-a"}));
    links.push(serde_json::json!({"op": "memberForgotten", "node": "local-a"}));
    let taken = take(&facade, &serde_json::json!({"from": "edge-2", "links": links}));
    assert_eq!(taken.links, ops);
    assert!(!taken.hosts);
}

/// Admission counts the members another edge member's fresh row names: never this member's own row, nor a row whose
/// heartbeat is older than [`MEMBER_LINK_FRESH_MS`]; a member's row goes when it stops, and one gone a day is pruned.
#[test]
fn only_another_edge_members_fresh_row_counts() {
    let (ctx, facade) = open();
    let held = |members: &[&str]| -> HeldLinks {
        HeldLinks::from([(
            "local-a".to_string(),
            members.iter().map(|member| (*member).to_string()).collect(),
        )])
    };
    write_member_links(&facade, "edge-1", &held(&["own"])).unwrap();
    write_member_links(&facade, "edge-2", &held(&["m-2"])).unwrap();
    write_member_links(&facade, "edge-3", &held(&["m-3"])).unwrap();
    let age = |edge: &str, millis: i64| {
        let mut row = facade.make_value("moqui.edge.EdgeMemberLinks").unwrap();
        row.set("edgeInstanceId", moqui_lib::object::Value::string(edge));
        row.set(
            "heartbeatDate",
            crate::user::stamp_value(crate::cluster::db_now(&facade) - millis),
        );
        row.update(&facade).unwrap();
    };
    age("edge-3", MEMBER_LINK_FRESH_MS + 1_000);
    let elsewhere = linked_elsewhere(ctx.factory(), &facade, "edge-1", "local-a").unwrap();
    assert_eq!(
        elsewhere.members,
        vec!["m-2".to_string()],
        "not its own, not a stale one"
    );
    assert!(linked_elsewhere(ctx.factory(), &facade, "edge-1", "local-b")
        .unwrap()
        .members
        .is_empty());
    // edge-2 stops; edge-3 has been gone a day
    age("edge-3", MEMBER_LINK_PRUNE_MS + 1_000);
    remove_member_links(&facade, Some("edge-2")).unwrap();
    let left = facade
        .find("moqui.edge.EdgeMemberLinks")
        .unwrap()
        .use_cache(false)
        .list()
        .unwrap()
        .len();
    assert_eq!(left, 1, "only edge-1's row is left");
}
