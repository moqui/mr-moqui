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

//! Pools of local nodes and failover to a standby (plans/Edge-Node2.md §4.4, §4.5, E17 part A). The edge node is a
//! real server (`node_role=front_edge`); the members are dialers with explicit configs sharing one pairing's key, each
//! naming its own instance id, with a [`FixedMember`] for its state and its cluster's member list (no server), except
//! where a delivery must be applied, which a real local server does (`local_web`).
//!
//! - Members: two members of one pairing both link, each with its own generation from the pairing's one counter; a
//!   member that dials again replaces only its own link; the member list is kept in memory and reported, members live
//!   in the cluster and not linked among them.
//! - Admission: a node the linked member's list does not have is refused "join the cluster first" and shown; once the
//!   list has it, it is admitted at once (the linked member is asked); with none linked the key admits a member.
//! - Drain: the edge node drains a member down its link and its state shows; a member's own state change is told at
//!   once.
//! - Failover: a new node taking a pairing over with none linked is admitted, noted on the pairing's row and told to
//!   the administrators; the first machine coming back is refused; a pairing set to hold a new node refuses it "held
//!   for approval" until it is approved.
//! - Deliver again since: delivered items go again, and the local node's receipts (by the pairing, from any member) make
//!   the repeat do nothing.

#![cfg(all(unix, feature = "edge", feature = "embedded-postgres"))]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use moqui_core::edge::{LinkState, MemberState};
use moqui_host::edge_link::{connect_up, local_web, pair, Dialer, FixedMember, LinkError, LocalConfig, PoolMember};
use moqui_lib::object::{to_plain_string, Value};
use tokio::sync::watch;

use crate::common::Gate;
use crate::edge_link::{call, certs, client, code, pair_config, quick, runtime, start_edge, wait_status, wait_until};

/// Each member of get#EdgeMembers for `node`, by instance id, as text.
fn members(gate: &Gate, node: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    let out = call(gate, "get#EdgeMembers", &[("pairedNodeId", node)]).unwrap();
    let Some(Value::List(list)) = out.get("memberList") else {
        panic!("memberList: {out:?}")
    };
    list.iter()
        .filter_map(|row| {
            let Value::Map(row) = row else { return None };
            let row: BTreeMap<String, String> = row
                .iter()
                .map(|(name, value)| (name.clone(), to_plain_string(value)))
                .collect();
            Some((row["instanceId"].clone(), row))
        })
        .collect()
}

/// The `field` of member `member` of `node` in get#EdgeMembers, empty when it is not there.
fn member_field(gate: &Gate, node: &str, member: &str, field: &str) -> String {
    members(gate, node)
        .get(member)
        .and_then(|row| row.get(field).cloned())
        .unwrap_or_default()
}

/// A member's config: the pairing's key, its own instance id, and a fixed state and cluster list.
fn member_config(
    url: &str,
    paired: &moqui_host::edge_link::Paired,
    id: &str,
    cluster: &[(&str, bool)],
) -> (LocalConfig, Arc<FixedMember>) {
    let fixed = FixedMember::new(id);
    fixed.set_members(cluster);
    let mut config = LocalConfig::new(url, &paired.key, &paired.pin, id);
    quick(&mut config);
    config.member = Some(Arc::clone(&fixed) as Arc<dyn PoolMember>);
    (config, fixed)
}

fn refused_with(outcome: Result<moqui_host::edge_link::UpLink, LinkError>, text: &str) {
    match outcome {
        Err(LinkError::Refused(message)) => assert!(message.contains(text), "{message}"),
        Err(other) => panic!("not refused but {other:?}"),
        Ok(_) => panic!("admitted; expected a refusal with {text:?}"),
    }
}

#[test]
fn the_members_of_a_pairing_link_and_one_not_in_their_cluster_is_refused() {
    if !crate::common::own_process() {
        return;
    }
    let certs = certs();
    let port = crate::common::free_port();
    let gate = start_edge("edge-pool", &certs, port);
    let url = format!("https://localhost:{port}");
    let rt = runtime();
    let paired = rt
        .block_on(pair(&pair_config(&url, &code(&gate), "member-a", &certs)))
        .unwrap();
    let cluster = [
        ("member-a", true),
        ("member-b", true),
        ("member-c", true),
        ("member-old", false),
    ];
    let (config_a, fixed_a) = member_config(&url, &paired, "member-a", &cluster);
    let (_shutdown, shutdown) = watch::channel(false);
    let dialer_a = Dialer::start(config_a, rt.handle(), shutdown.clone(), None).unwrap();
    let linked = |status: &moqui_core::edge::LinkStatus| status.state == LinkState::Linked;
    let up_a = wait_status(&dialer_a, Duration::from_secs(10), "a linked", linked);
    wait_until(Duration::from_secs(5), "a's down connection", || {
        client(&gate, "member-a")["down"] == "true"
    });

    // the edge node keeps a's member list in memory and reports from it: b and c live in the cluster and not linked
    let report = members(&gate, "member-a");
    assert_eq!(report["member-a"]["condition"], "ok", "{report:?}");
    assert_eq!(report["member-a"]["state"], "live");
    assert_eq!(report["member-b"]["condition"], "notLinked", "{report:?}");
    assert_eq!(report["member-c"]["condition"], "notLinked");
    assert_eq!(report["member-old"]["condition"], "stopped");

    // b links beside a, each with its own generation from the pairing's one counter
    let (config_b, _fixed_b) = member_config(&url, &paired, "member-b", &cluster);
    let dialer_b = Dialer::start(config_b.clone(), rt.handle(), shutdown.clone(), None).unwrap();
    let up_b = wait_status(&dialer_b, Duration::from_secs(10), "b linked", linked);
    assert_ne!(up_a.generation, up_b.generation);
    wait_until(Duration::from_secs(5), "both members linked", || {
        client(&gate, "member-a")["membersLinked"] == "2"
    });
    assert_eq!(member_field(&gate, "member-a", "member-b", "condition"), "ok");

    // b dialing again while its link still answers is refused (plans/Edge-Node2.md §4.11)
    match rt.block_on(connect_up(&config_b)) {
        Err(LinkError::Failed(message)) => assert!(message.contains("answers"), "{message}"),
        Err(other) => panic!("not a retryable refusal: {other:?}"),
        Ok(_) => panic!("a second link of b was taken while its link answers"),
    }
    // a node with the pairing's key naming a's id, or b's, with another key of its own (a stolen .env): refused
    for id in ["member-a", "member-b"] {
        let (thief, _) = member_config(&url, &paired, id, &cluster);
        refused_with(rt.block_on(connect_up(&thief)), "join the cluster first");
    }
    // a down connection names its link's token as well as its generation: a guessed generation takes nothing
    let generation_b = dialer_b.status().generation;
    let guessed = rt.block_on(moqui_host::edge_link::dial::try_down(
        &config_b,
        generation_b,
        &[0; moqui_host::edge_link::DOWN_TOKEN],
    ));
    assert!(guessed.is_err(), "a down connection was taken with a guessed token");
    std::thread::sleep(Duration::from_millis(600));
    for (dialer, generation) in [(&dialer_a, up_a.generation), (&dialer_b, generation_b)] {
        let status = dialer.status();
        assert_eq!(
            (status.state, status.generation, status.reconnects),
            (LinkState::Linked, generation, 0),
            "a member's link was touched: {status:?}"
        );
    }

    // a node with the pairing's key that a's cluster does not have: refused, and shown
    let (config_z, _) = member_config(&url, &paired, "member-z", &[("member-z", true)]);
    refused_with(rt.block_on(connect_up(&config_z)), "join the cluster first");
    assert_eq!(member_field(&gate, "member-a", "member-z", "condition"), "refused");
    // once a's cluster has it live it is admitted at once: a linked member is asked for its list then
    let mut joined = cluster.to_vec();
    joined.push(("member-z", true));
    fixed_a.set_members(&joined);
    _fixed_b.set_members(&joined);
    let z = rt.block_on(connect_up(&config_z)).unwrap();
    rt.block_on(z.ping()).unwrap();
    // z dials again with no down connection on its first link: only its own older link is replaced
    let z_again = rt.block_on(connect_up(&config_z)).unwrap();
    assert!(z_again.generation > z.generation);
    assert!(rt.block_on(z.ping()).is_err(), "z's older link still answers");
    rt.block_on(z_again.ping()).unwrap();
    assert_eq!(dialer_a.status().generation, up_a.generation);
    drop((z, z_again));

    // drain: the edge node asks a down its link; a is draining, and the pairing's work goes to b
    let drained = call(
        &gate,
        "drain#EdgeMember",
        &[
            ("pairedNodeId", "member-a"),
            ("instanceId", "member-a"),
            ("drain", "true"),
        ],
    )
    .unwrap();
    assert_eq!(to_plain_string(&drained["state"]), "draining");
    assert_eq!(fixed_a.status().0, MemberState::Draining);
    assert_eq!(member_field(&gate, "member-a", "member-a", "state"), "draining");
    for _ in 0..3 {
        call(&gate, "ping#EdgeClient", &[("pairedNodeId", "member-a")]).unwrap();
    }
    // Clients' Ping on a member row reaches that member, draining or not; a member not linked has nowhere to go
    let ping = call(
        &gate,
        "ping#EdgeClient",
        &[("pairedNodeId", "member-a"), ("instanceId", "member-a")],
    )
    .unwrap();
    assert!(matches!(ping["rttMs"], Value::F64(rtt) if rtt >= 0.0), "{ping:?}");
    let stranger = call(
        &gate,
        "ping#EdgeClient",
        &[("pairedNodeId", "member-a"), ("instanceId", "member-q")],
    );
    // a failed service answers no rttMs, its error in the context's messages
    assert!(
        stranger.as_ref().map_or(true, |out| !out.contains_key("rttMs")),
        "{stranger:?}"
    );
    let resumed = call(
        &gate,
        "drain#EdgeMember",
        &[
            ("pairedNodeId", "member-a"),
            ("instanceId", "member-a"),
            ("drain", "false"),
        ],
    )
    .unwrap();
    assert_eq!(to_plain_string(&resumed["state"]), "live");

    // the member's own state is told at once, not at the next Ping
    fixed_a.set_state(MemberState::Starting);
    dialer_a.state_changed();
    wait_until(Duration::from_secs(2), "a starting", || {
        member_field(&gate, "member-a", "member-a", "state") == "starting"
    });
    fixed_a.set_state(MemberState::Live);
    dialer_a.state_changed();

    // none linked: the key admits a member of the kept list, which is no takeover
    dialer_a.stop();
    dialer_b.stop();
    wait_until(Duration::from_secs(5), "the pool unlinked", || {
        client(&gate, "member-a")["linked"] == "false"
    });
    let back = rt.block_on(connect_up(&config_b)).unwrap();
    rt.block_on(back.ping()).unwrap();
    assert_eq!(client(&gate, "member-a")["takeoverToId"], "", "a pool member came back");
}

/// The takeover notices on the edge node.
fn takeover_notices(gate: &Gate) -> u64 {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    facade
        .find("moqui.security.user.NotificationMessage")
        .unwrap()
        .eq("topic", Value::string("EdgeMemberTakeover"))
        .count()
        .unwrap()
        .try_into()
        .unwrap()
}

#[test]
fn a_new_node_takes_a_pairing_over_with_a_notice_or_is_held_for_approval() {
    if !crate::common::own_process() {
        return;
    }
    let certs = certs();
    let port = crate::common::free_port();
    let gate = start_edge("edge-takeover", &certs, port);
    let url = format!("https://localhost:{port}");
    let rt = runtime();
    let paired = rt
        .block_on(pair(&pair_config(&url, &code(&gate), "first", &certs)))
        .unwrap();
    let unlinked = |what: &str| {
        wait_until(Duration::from_secs(5), what, || {
            client(&gate, "first")["linked"] == "false"
        });
    };
    let (config_first, _) = member_config(&url, &paired, "first", &[("first", true)]);
    let first = rt.block_on(connect_up(&config_first)).unwrap();
    rt.block_on(first.ping()).unwrap();
    drop(first);
    unlinked("the first machine unlinked");
    assert_eq!(takeover_notices(&gate), 0);

    // with the pairing holding new nodes, a node naming the first machine's id with another key (a stolen .env) is a new
    // node: held, with a notice; the first machine, with its own key, comes back
    call(&gate, "hold#NewMembers", &[("pairedNodeId", "first"), ("hold", "true")]).unwrap();
    let (thief, _) = member_config(&url, &paired, "first", &[("first", true)]);
    refused_with(rt.block_on(connect_up(&thief)), "held for approval");
    assert_eq!(takeover_notices(&gate), 1);
    let first = rt.block_on(connect_up(&config_first)).unwrap();
    drop(first);
    unlinked("the first machine unlinked again");
    call(
        &gate,
        "hold#NewMembers",
        &[("pairedNodeId", "first"), ("hold", "false")],
    )
    .unwrap();

    // a standby with a new instance id: admitted with none linked, noted on the pairing and told to the administrators
    let (config_standby, _) = member_config(&url, &paired, "standby", &[("standby", true)]);
    let standby = rt.block_on(connect_up(&config_standby)).unwrap();
    wait_until(Duration::from_secs(5), "the takeover noted", || {
        client(&gate, "first")["takeoverToId"] == "standby"
    });
    let row = client(&gate, "first");
    assert_eq!(
        (row["takeoverFromId"].as_str(), row["takeoverToId"].as_str()),
        ("first", "standby"),
        "{row:?}"
    );
    assert!(!row["takeoverDate"].is_empty());
    wait_until(Duration::from_secs(5), "the takeover told", || {
        takeover_notices(&gate) == 2
    });

    // the first machine, started again while the standby is linked, is refused; so is the standby's id with another key
    refused_with(rt.block_on(connect_up(&config_first)), "taken over");
    assert_eq!(member_field(&gate, "first", "first", "condition"), "refused");
    let (standby_thief, _) = member_config(&url, &paired, "standby", &[("standby", true)]);
    refused_with(rt.block_on(connect_up(&standby_thief)), "join the cluster first");
    // and with the standby's link down too (a blip, an edge node restart): it never takes the pairing back by itself
    drop(standby);
    unlinked("the standby unlinked");
    refused_with(rt.block_on(connect_up(&config_first)), "taken over");
    // until its operator clears the takeover
    call(&gate, "clear#EdgeTakeover", &[("pairedNodeId", "first")]).unwrap();
    let first = rt.block_on(connect_up(&config_first)).unwrap();
    rt.block_on(first.ping()).unwrap();
    drop(first);
    unlinked("the first machine unlinked once more");

    // held for approval: a new node is refused until approved, then admitted as a takeover
    call(&gate, "hold#NewMembers", &[("pairedNodeId", "first"), ("hold", "true")]).unwrap();
    let (config_third, _) = member_config(&url, &paired, "third", &[("third", true)]);
    refused_with(rt.block_on(connect_up(&config_third)), "held for approval");
    assert_eq!(member_field(&gate, "first", "third", "condition"), "held");
    assert_eq!(takeover_notices(&gate), 3, "the administrators are told of a node held");
    refused_with(rt.block_on(connect_up(&config_third)), "held for approval");
    assert_eq!(takeover_notices(&gate), 3, "told once");
    call(
        &gate,
        "approve#EdgeMember",
        &[("pairedNodeId", "first"), ("instanceId", "third")],
    )
    .unwrap();
    let third = rt.block_on(connect_up(&config_third)).unwrap();
    rt.block_on(third.ping()).unwrap();
    wait_until(Duration::from_secs(5), "the approved takeover noted", || {
        client(&gate, "first")["takeoverToId"] == "third"
    });
    assert_eq!(
        client(&gate, "first")["approvedMemberId"],
        "",
        "an approval is used once"
    );
    wait_until(Duration::from_secs(5), "the takeover told", || {
        takeover_notices(&gate) == 4
    });
    drop(third);
    unlinked("the third unlinked");

    // the last member with a new key (its key forgotten, or its key file made again): a takeover from itself, which
    // displaces no one, so it comes back after with that key and is not refused as taken over
    call(
        &gate,
        "hold#NewMembers",
        &[("pairedNodeId", "first"), ("hold", "false")],
    )
    .unwrap();
    call(
        &gate,
        "forget#EdgeMember",
        &[("pairedNodeId", "first"), ("instanceId", "third")],
    )
    .unwrap();
    let (third_new_key, _) = member_config(&url, &paired, "third", &[("third", true)]);
    let third = rt.block_on(connect_up(&third_new_key)).unwrap();
    rt.block_on(third.ping()).unwrap();
    drop(third);
    unlinked("the third unlinked again");
    assert_ne!(client(&gate, "first")["takeoverFromId"], "third", "it displaced itself");
    let third = rt.block_on(connect_up(&third_new_key)).unwrap();
    rt.block_on(third.ping()).unwrap();
    // forgetting a linked member's key closes its link
    call(
        &gate,
        "forget#EdgeMember",
        &[("pairedNodeId", "first"), ("instanceId", "third")],
    )
    .unwrap();
    unlinked("the forgotten member's link closed");
    drop(third);

    // two new nodes at once with none linked: one is admitted, never both (plans/Edge-Node2.md §4.11)
    call(
        &gate,
        "hold#NewMembers",
        &[("pairedNodeId", "first"), ("hold", "false")],
    )
    .unwrap();
    let (config_x, _) = member_config(&url, &paired, "new-x", &[("new-x", true)]);
    let (config_y, _) = member_config(&url, &paired, "new-y", &[("new-y", true)]);
    let (x, y) = rt.block_on(async { tokio::join!(connect_up(&config_x), connect_up(&config_y)) });
    assert_eq!(
        usize::from(x.is_ok()) + usize::from(y.is_ok()),
        1,
        "x {:?}, y {:?}",
        x.as_ref().map(|up| up.generation),
        y.as_ref().map(|up| up.generation)
    );
    // the notices of a pairing are capped in an hour: cycling ids does not flood the administrators
    assert_eq!(takeover_notices(&gate), 4);
}

/// `deliver#ItemsAgain` on the edge node, as an administrator.
fn deliver_again(edge: &Gate, node: &str, since: i64) -> BTreeMap<String, Value> {
    let factory = edge.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let _authz = context.disable_authz();
    let parameters = BTreeMap::from([
        ("pairedNodeId".to_string(), Value::string(node)),
        ("sinceDate".to_string(), moqui_core::user::stamp_value(since)),
    ]);
    moqui_core::service::sync("org.moqui.edge.EdgeServices.deliver#ItemsAgain", &parameters).unwrap()
}

/// The edge node's item `id`: its attempts, and whether a next attempt is set.
fn attempts(edge: &Gate, id: &str) -> (i64, bool) {
    let factory = edge.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    let row = facade
        .find("moqui.edge.EdgeQueueItem")
        .unwrap()
        .eq("queueItemId", Value::string(id))
        .one()
        .unwrap()
        .unwrap();
    let count = row
        .get("attemptCount")
        .map(to_plain_string)
        .and_then(|text| text.parse().ok())
        .unwrap_or(0);
    let next = row
        .get("nextAttemptDate")
        .is_some_and(|value| !to_plain_string(value).is_empty());
    (count, next)
}

/// The local node's receipt of item `id`, by the pairing: its appliedDate, if any.
fn receipt(local: &Gate, pairing: &str, id: &str) -> Option<String> {
    let factory = local.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    facade
        .find("moqui.edge.EdgeReceipt")
        .unwrap()
        .eq("pairedNodeId", Value::string(pairing))
        .eq("queueItemId", Value::string(id))
        .one()
        .unwrap()
        .map(|row| row.get("appliedDate").map(to_plain_string).unwrap_or_default())
}

/// The edge node's item `id`: its status and its last error.
fn item(edge: &Gate, id: &str) -> (String, String) {
    let factory = edge.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    let row = facade
        .find("moqui.edge.EdgeQueueItem")
        .unwrap()
        .eq("queueItemId", Value::string(id))
        .one()
        .unwrap()
        .unwrap();
    (
        row.get("statusEnumId").map(to_plain_string).unwrap_or_default(),
        row.get("lastErrorText").map(to_plain_string).unwrap_or_default(),
    )
}

#[test]
fn delivered_items_go_again_and_the_receipts_make_the_repeat_do_nothing() {
    if !crate::common::own_process() {
        return;
    }
    let certs = certs();
    // the local node keeps its queue rows and receipts by the pairing its link names, whichever member applies; an
    // edge_node_pairing that says another is logged and written again
    let local =
        crate::common::start_with_properties("edge-again-local", None, &[("edge_node_pairing", "stale-pairing")]);
    let port = crate::common::free_port();
    let port_text = port.to_string();
    let edge = crate::common::start_second_with(
        "edge-again-edge",
        "",
        &[("node_role", "front_edge"), ("webapp_https_listen_port", &port_text)],
        &[
            ("cert/default.crt", &certs.cert_pem),
            ("cert/default.key", &certs.key_pem),
        ],
    );
    let url = format!("https://localhost:{port}");
    let rt = runtime();
    let paired = rt
        .block_on(pair(&pair_config(&url, &code(&edge), "pool-a", &certs)))
        .unwrap();
    let since = moqui_core::user::now_millis() - 1_000;
    let payload = serde_json::json!({"token": "again-1", "back": true, "ok": true, "message": "once"}).to_string();
    let queued = call(
        &edge,
        "enqueue#Item",
        &[
            ("kind", "edgeEcho"),
            ("conversationKey", "again"),
            ("payload", &payload),
            ("pairedNodeId", "pool-a"),
        ],
    )
    .unwrap();
    let id = to_plain_string(&queued["queueItemId"]);
    let start = |member: &str| {
        let mut config = LocalConfig::new(&url, &paired.key, &paired.pin, member);
        quick(&mut config);
        config.web = local_web(local.server.factory());
        assert!(config.web.is_some(), "the local server's app");
        let (_stop, shutdown) = watch::channel(false);
        let dialer = Dialer::start(config, rt.handle(), shutdown, None).unwrap();
        wait_status(&dialer, Duration::from_secs(10), "linked", |status| {
            status.state == LinkState::Linked
        });
        (dialer, _stop)
    };
    let (first, _first_stop) = start("pool-a");
    wait_until(Duration::from_secs(10), "the item applied", || {
        receipt(&local, "pool-a", &id).is_some()
    });
    wait_until(Duration::from_secs(5), "the item delivered", || {
        item(&edge, &id).0 == "EqsDelivered"
    });
    let applied = receipt(&local, "pool-a", &id).unwrap();
    let env = std::fs::read_to_string(local.runtime.join(".env")).unwrap_or_default();
    assert!(env.contains("edge_node_pairing=pool-a"), "{env}");
    first.stop();
    wait_until(Duration::from_secs(5), "the first member unlinked", || {
        client(&edge, "pool-a")["linked"] == "false"
    });

    // a standby on the same data takes over, and the items delivered since go again: its receipts are the pairing's,
    // so the repeat is answered as applied before and nothing runs twice
    let (_standby, _standby_stop) = start("pool-standby");
    // a member draining claims nothing: its delivery waits, the item's attempts untouched, until it resumes
    let drain = |drain: &str| {
        call(
            &edge,
            "drain#EdgeMember",
            &[
                ("pairedNodeId", "pool-a"),
                ("instanceId", "pool-standby"),
                ("drain", drain),
            ],
        )
        .unwrap()
    };
    assert_eq!(to_plain_string(&drain("true")["state"]), "draining");
    let payload = serde_json::json!({"token": "again-2", "back": true, "ok": true}).to_string();
    let waiting = call(
        &edge,
        "enqueue#Item",
        &[
            ("kind", "edgeEcho"),
            ("conversationKey", "again"),
            ("payload", &payload),
            ("pairedNodeId", "pool-a"),
        ],
    )
    .unwrap();
    let waiting = to_plain_string(&waiting["queueItemId"]);
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(item(&edge, &waiting).0, "EqsPending");
    assert_eq!(attempts(&edge, &waiting), (0, false), "a draining member took the item");
    assert_eq!(to_plain_string(&drain("false")["state"]), "live");
    wait_until(Duration::from_secs(10), "the waiting item delivered", || {
        item(&edge, &waiting).0 == "EqsDelivered"
    });
    let again = deliver_again(&edge, "pool-a", since);
    assert_eq!(to_plain_string(&again["itemCount"]), "2", "{again:?}");
    wait_until(Duration::from_secs(10), "the item delivered again", || {
        let (status, error) = item(&edge, &id);
        status == "EqsDelivered" && error.contains("delivered again")
    });
    assert_eq!(
        receipt(&local, "pool-a", &id).as_deref(),
        Some(applied.as_str()),
        "the repeat was applied again"
    );
    // nothing delivered since now
    let none = deliver_again(&edge, "pool-a", moqui_core::user::now_millis() + 60_000);
    assert_eq!(to_plain_string(&none["itemCount"]), "0");
}

// ===== acceptance (plans/Edge-Node2.md §5, E17): real local servers, each with its own database unless named =====

/// The cluster directory a second member's child process adopts; set only for that child.
const MEMBER_ENV: &str = "MOQUI_EDGE_POOL_MEMBER_CLUSTER";
const MEMBER_TEST: &str = "edge_pool::pool_member_process";
/// The generated secrets a second member of the cluster takes from the first.
const SHARED_SECRETS: [&str; 4] = [
    "session_cookie_key",
    "cluster_tls_cert",
    "cluster_tls_key",
    "entity_ds_crypt_pass",
];

/// A second member of a local server's cluster, on its database, in a child process of this test binary: its member
/// row and heartbeat are real. Dropping it closes the child's stdin, which stops it.
struct ChildMember {
    child: std::process::Child,
    node: String,
}

impl ChildMember {
    fn start(cluster_dir: &std::path::Path) -> ChildMember {
        use std::io::BufRead as _;
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        for name in SHARED_SECRETS {
            let value = moqui_lib::env::get_prop_or_env(name);
            if !value.is_empty() {
                command.env(name, value);
            }
        }
        let mut child = command
            .args([MEMBER_TEST, "--exact", "--nocapture", "--test-threads", "1"])
            .env("MOQUI_TEST_OWN_PROCESS", MEMBER_TEST)
            .env(MEMBER_ENV, cluster_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the member's process");
        let stdout = child.stdout.take().unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut lines = std::io::BufReader::new(stdout).lines().map_while(Result::ok);
            let ready = lines
                .by_ref()
                .find_map(|line| line.find("POOL_MEMBER ").map(|at| line[at + 12..].trim().to_string()));
            let _ = ready_tx.send(ready);
            lines.for_each(drop);
        });
        match ready_rx.recv_timeout(Duration::from_secs(30)) {
            Ok(Some(node)) => ChildMember { child, node },
            outcome => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the second member did not start: {outcome:?}");
            }
        }
    }
}

impl Drop for ChildMember {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let until = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < until && matches!(self.child.try_wait(), Ok(None)) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The second member's process: a server on the first's database, until its stdin closes.
#[test]
fn pool_member_process() {
    let Some(cluster_dir) = std::env::var_os(MEMBER_ENV) else {
        return;
    };
    if !crate::common::own_process() {
        return;
    }
    let (server, runtime) = crate::common::start_member("edge-pool-member", std::path::Path::new(&cluster_dir), &[]);
    let node = moqui_core::cluster::instance_id(server.factory()).unwrap();
    println!("POOL_MEMBER {node}");
    std::io::Write::flush(&mut std::io::stdout()).unwrap();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    drop(server);
    drop(runtime);
}

/// The edge node after the local servers (`node_role` is a process property they must not read), and the pairing of
/// `local` under its own instance id: the edge node, the pairing's key, and its URL.
fn edge_for(
    label: &str,
    local: &Gate,
    certs: &crate::edge_link::Certs,
) -> (Gate, moqui_host::edge_link::Paired, String) {
    let port = crate::common::free_port();
    let port_text = port.to_string();
    let edge = crate::common::start_second_with(
        label,
        "",
        &[("node_role", "front_edge"), ("webapp_https_listen_port", &port_text)],
        &[
            ("cert/default.crt", &certs.cert_pem),
            ("cert/default.key", &certs.key_pem),
        ],
    );
    let url = format!("https://localhost:{port}");
    let node = moqui_core::cluster::instance_id(local.server.factory()).unwrap();
    let paired = runtime()
        .block_on(pair(&pair_config(&url, &code(&edge), &node, certs)))
        .unwrap();
    (edge, paired, url)
}

/// A dialer for server `local` under its own instance id, with the pairing's key.
fn server_dialer(
    rt: &tokio::runtime::Runtime,
    local: &Gate,
    paired: &moqui_host::edge_link::Paired,
    url: &str,
) -> (LocalConfig, Dialer, watch::Sender<bool>) {
    let node = moqui_core::cluster::instance_id(local.server.factory()).unwrap();
    let mut config = LocalConfig::new(url, &paired.key, &paired.pin, &node);
    quick(&mut config);
    config.web = local_web(local.server.factory());
    // its own key, kept in its runtime as the hook keeps it: a machine started again is the same member
    config.member_key = Some(moqui_host::edge_link::MemberKey::load_or_create(&local.runtime).unwrap());
    let (stop, shutdown) = watch::channel(false);
    let dialer = Dialer::start(config.clone(), rt.handle(), shutdown, None).unwrap();
    (config, dialer, stop)
}

#[test]
#[ignore = "acceptance: Clients shows a cluster member that is live but not linked (Edge-Node2 §5, E17)"]
fn clients_shows_a_cluster_member_that_is_live_but_not_linked() {
    if !crate::common::own_process() {
        return;
    }
    let certs = certs();
    let cluster_port = crate::common::free_port().to_string();
    let local =
        crate::common::start_with_properties("edge-pool-cluster", None, &[("cluster_https_port", &cluster_port)]);
    let second = ChildMember::start(&local.cluster_dir);
    let (edge, paired, url) = edge_for("edge-pool-cluster-edge", &local, &certs);
    let rt = runtime();
    let (config, dialer, _stop) = server_dialer(&rt, &local, &paired, &url);
    wait_status(&dialer, Duration::from_secs(10), "the first member linked", |status| {
        status.state == LinkState::Linked
    });
    // its list is its cluster's member rows: the second member, live there, is shown live and not linked
    wait_until(Duration::from_secs(10), "the second member shown", || {
        member_field(&edge, &config.instance_id, &second.node, "condition") == "notLinked"
    });
    let report = members(&edge, &config.instance_id);
    assert_eq!(report[&config.instance_id]["condition"], "ok", "{report:?}");
    assert_eq!(report[&second.node]["liveInCluster"], "true");
}

#[test]
#[ignore = "acceptance: a node on another database with the same key is refused while a member is linked (Edge-Node2 §5, E17)"]
fn a_node_on_another_database_with_the_same_key_is_refused_while_a_member_is_linked() {
    if !crate::common::own_process() {
        return;
    }
    let certs = certs();
    let local = crate::common::start_with_properties("edge-pool-prod", None, &[]);
    // a staging copy with production's .env: its own database and instance id, the same key
    let copy = crate::common::start_second("edge-pool-copy", None, "");
    let (edge, paired, url) = edge_for("edge-pool-prod-edge", &local, &certs);
    let rt = runtime();
    let (config, dialer, _stop) = server_dialer(&rt, &local, &paired, &url);
    wait_status(&dialer, Duration::from_secs(10), "production linked", |status| {
        status.state == LinkState::Linked
    });
    wait_until(Duration::from_secs(5), "production's down connection", || {
        client(&edge, &config.instance_id)["down"] == "true"
    });
    let copy_id = moqui_core::cluster::instance_id(copy.server.factory()).unwrap();
    let mut copy_config = LocalConfig::new(&url, &paired.key, &paired.pin, &copy_id);
    quick(&mut copy_config);
    copy_config.web = local_web(copy.server.factory());
    copy_config.member_key = Some(moqui_host::edge_link::MemberKey::load_or_create(&copy.runtime).unwrap());
    refused_with(rt.block_on(connect_up(&copy_config)), "join the cluster first");
    assert_eq!(
        member_field(&edge, &config.instance_id, &copy_id, "condition"),
        "refused"
    );
    let status = dialer.status();
    assert_eq!((status.state, status.reconnects), (LinkState::Linked, 0), "{status:?}");
}

#[test]
#[ignore = "acceptance: a standby started by hand takes over and gets the queued items once; the first machine is refused (Edge-Node2 §5, E17)"]
fn a_standby_takes_over_gets_the_queued_items_once_and_the_first_machine_is_refused() {
    if !crate::common::own_process() {
        return;
    }
    let certs = certs();
    let first = crate::common::start_with_properties("edge-pool-first", None, &[]);
    // the standby: the same runtime and .env on another machine, here a database of its own (a restored backup)
    let standby = crate::common::start_second("edge-pool-standby", None, "");
    let (edge, paired, url) = edge_for("edge-pool-failover-edge", &first, &certs);
    let pairing = moqui_core::cluster::instance_id(first.server.factory()).unwrap();
    // the .env both machines share names the pairing (written at pairing), which their queue rows and receipts use
    moqui_lib::env::set_property(moqui_core::edge::NODE_PAIRING, pairing.clone());
    let rt = runtime();
    let (_, dialer, _stop) = server_dialer(&rt, &first, &paired, &url);
    wait_status(&dialer, Duration::from_secs(10), "the first machine linked", |status| {
        status.state == LinkState::Linked
    });
    // the first machine dies; the edge node holds what comes for it meanwhile
    dialer.stop();
    drop(dialer);
    wait_until(Duration::from_secs(5), "the first machine unlinked", || {
        client(&edge, &pairing)["linked"] == "false"
    });
    let mut ids = Vec::new();
    for n in 0..3 {
        let payload = serde_json::json!({"token": format!("away-{n}"), "back": true, "ok": true}).to_string();
        let queued = call(
            &edge,
            "enqueue#Item",
            &[
                ("kind", "edgeEcho"),
                ("conversationKey", "away"),
                ("payload", &payload),
                ("pairedNodeId", &pairing),
            ],
        )
        .unwrap();
        ids.push(to_plain_string(&queued["queueItemId"]));
    }
    let notices = takeover_notices(&edge);

    // the standby, started by hand, takes over its names, with the notice, and gets the queued items once
    let (standby_config, standby_dialer, _standby_stop) = server_dialer(&rt, &standby, &paired, &url);
    wait_status(
        &standby_dialer,
        Duration::from_secs(10),
        "the standby linked",
        |status| status.state == LinkState::Linked,
    );
    wait_until(Duration::from_secs(5), "the takeover noted and told", || {
        client(&edge, &pairing)["takeoverToId"] == standby_config.instance_id && takeover_notices(&edge) == notices + 1
    });
    wait_until(Duration::from_secs(15), "the queued items delivered", || {
        ids.iter().all(|id| item(&edge, id).0 == "EqsDelivered")
    });
    for id in &ids {
        assert!(
            receipt(&standby, &pairing, id).is_some(),
            "{id} not applied on the standby"
        );
        assert!(
            receipt(&first, &pairing, id).is_none(),
            "{id} applied on the first machine"
        );
    }

    // the first machine, started again, is refused: the standby took the pairing over from it
    let (_, again, _again_stop) = server_dialer(&rt, &first, &paired, &url);
    let refused = wait_status(&again, Duration::from_secs(10), "the first machine refused", |status| {
        status.state == LinkState::Refused
    });
    assert!(refused.last_error.contains("taken over"), "{refused:?}");
    let standby_status = standby_dialer.status();
    assert_eq!(standby_status.state, LinkState::Linked, "{standby_status:?}");
}
