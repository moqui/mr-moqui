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

//! The edge mode's recipients from the replicated copies: each local node's own, by account and identity address and
//! plus tag, without the accounts the sync feeds or that are removed. The sessions and the queue are
//! `tests/it/edge_mail.rs`.

use super::*;
use serde_json::json;

const ACCOUNT: &str = "moqui.basic.email.EmailAccount";
const IDENTITY: &str = "moqui.basic.email.EmailIdentity";
const SERVER: &str = "moqui.basic.email.EmailServer";

fn copy(node: &str, entity: &str, pk: serde_json::Value, fields: serde_json::Value) -> replica::ReplicaValue {
    replica::ReplicaValue {
        paired_node_id: node.to_string(),
        set_name: RECIPIENT_SET.to_string(),
        entity_name: entity.to_string(),
        pk,
        fields,
    }
}

fn account(node: &str, id: &str, address: &str, kind: &str, server: &str) -> replica::ReplicaValue {
    copy(
        node,
        ACCOUNT,
        json!({"emailAccountId": id}),
        json!({"emailAddress": address, "accountKindEnumId": kind, "statusId": "EasActive", "emailServerId": server}),
    )
}

#[test]
fn the_recipients_of_each_node_come_from_its_copies() {
    let values = vec![
        account("node-a", "100", "Sales@Example.com", "EakHosted", ""),
        account("node-a", "101", "team@example.com", "EakShared", "IMAP1"),
        account("node-a", "102", "alice@example.com", "EakExternal", ""),
        copy(
            "node-a",
            ACCOUNT,
            json!({"emailAccountId": "103"}),
            json!({"emailAddress": "old@example.com", "accountKindEnumId": "EakHosted", "statusId": "EasRemoved"}),
        ),
        copy(
            "node-a",
            SERVER,
            json!({"emailServerId": "IMAP1"}),
            json!({"storeHost": "imap.example.com"}),
        ),
        copy(
            "node-a",
            IDENTITY,
            json!({"emailIdentityId": "9"}),
            json!({"emailAddress": "orders@example.com", "emailAccountId": "100"}),
        ),
        account("node-b", "100", "help@example.org", "EakHosted", "IMAP1"),
    ];
    let mut copies = Copies {
        built_at: Some(1),
        loaded: HashSet::from(["node-a".to_string(), "node-b".to_string()]),
        nodes: build(&values),
        active: HashSet::from(["node-a".to_string(), "node-b".to_string()]),
    };
    let found = |node: &str, account: &str| Lookup::Found {
        node: node.to_string(),
        account: account.to_string(),
    };
    assert_eq!(copies.find("sales@example.com", "+"), found("node-a", "100"));
    assert_eq!(copies.find("sales+news@example.com", "+"), found("node-a", "100"));
    assert_eq!(copies.find("ORDERS@example.com", "+"), found("node-a", "100"));
    assert_eq!(
        copies.find("team@example.com", "+"),
        Lookup::Unknown,
        "an account the sync feeds"
    );
    assert_eq!(
        copies.find("alice@example.com", "+"),
        Lookup::Unknown,
        "an external account"
    );
    assert_eq!(
        copies.find("old@example.com", "+"),
        Lookup::Unknown,
        "a removed account"
    );
    assert_eq!(
        copies.find("help@example.org", "+"),
        found("node-b", "100"),
        "another node's server of the same id has no store host there"
    );
    assert_eq!(copies.find("nobody@example.com", "+"), Lookup::Unknown);

    // a node whose set has not been sent whole yet: an address not found is not refused for good
    copies.loaded.remove("node-b");
    assert_eq!(copies.find("nobody@example.com", "+"), Lookup::NotLoaded);
    assert_eq!(
        copies.find("sales@example.com", "+"),
        found("node-a", "100"),
        "found is found"
    );
    // a node revoked: its copies are kept, but its mail is not taken
    copies.active.remove("node-a");
    assert_eq!(copies.find("sales@example.com", "+"), Lookup::NotPaired);
    copies.active.remove("node-b");
    assert_eq!(
        copies.find("nobody@example.com", "+"),
        Lookup::Unknown,
        "no paired node"
    );
}

/// Two pairings of one edge node (plans/Edge-Node2.md §4.1): each address goes to the pairing whose copies have it;
/// one both have is refused with 451 rather than given to either.
#[test]
fn an_address_two_pairings_hold_gets_451_and_each_other_goes_to_its_own() {
    let values = vec![
        account("node-a", "100", "ann@a.example", "EakHosted", ""),
        account("node-b", "100", "bob@b.example", "EakHosted", ""),
        account("node-a", "101", "Info@shared.example", "EakHosted", ""),
        account("node-b", "102", "info@shared.example", "EakShared", ""),
    ];
    let nodes = HashSet::from(["node-a".to_string(), "node-b".to_string()]);
    let mut copies = Copies {
        built_at: Some(1),
        loaded: nodes.clone(),
        nodes: build(&values),
        active: nodes,
    };
    let found = |node: &str, account: &str| Lookup::Found {
        node: node.to_string(),
        account: account.to_string(),
    };
    assert_eq!(copies.find("ann@a.example", "+"), found("node-a", "100"));
    assert_eq!(copies.find("bob+x@b.example", "+"), found("node-b", "100"));
    let held = copies.find("info@shared.example", "+");
    assert_eq!(held, Lookup::Held(vec!["node-a".to_string(), "node-b".to_string()]));
    assert!(matches!(lookup_answer(held), RcptAnswer::TempFail(_)), "451, not 550");
    // the most specific match wins: A's account at joe+x@ as it is over B's joe@ without the tag
    let values = vec![
        account("node-a", "200", "joe+x@d.example", "EakHosted", ""),
        account("node-b", "201", "joe@d.example", "EakHosted", ""),
    ];
    let both = Copies {
        nodes: build(&values),
        built_at: Some(2),
        loaded: copies.loaded.clone(),
        active: copies.active.clone(),
    };
    assert_eq!(both.find("joe+x@d.example", "+"), found("node-a", "200"));
    assert_eq!(both.find("joe+y@d.example", "+"), found("node-b", "201"));
    assert_eq!(both.find("joe@d.example", "+"), found("node-b", "201"));
    // once one pairing is revoked, the other's holds
    copies.active.remove("node-b");
    assert_eq!(copies.find("info@shared.example", "+"), found("node-a", "101"));
}

/// An all-in-one node's copies: paired node `node-a`, sent whole.
fn all_in_one_copies() -> Copies {
    let values = vec![
        account("node-a", "100", "ann@desk.example", "EakHosted", ""),
        account("node-a", "101", "both@server.example", "EakHosted", ""),
    ];
    Copies {
        built_at: Some(1),
        loaded: HashSet::from(["node-a".to_string()]),
        nodes: build(&values),
        active: HashSet::from(["node-a".to_string()]),
    }
}

/// The server's own account of `address`, as `moqui_mail::delivery::find_recipient` finds it with `delimiters`.
fn own_account(address: &str, delimiters: &str) -> Option<String> {
    moqui_mail::delivery::recipient_candidates(address, delimiters)
        .iter()
        .find_map(|candidate| match candidate.as_str() {
            "sales@server.example" => Some("100".to_string()),
            "both@server.example" => Some("100011".to_string()),
            "bob@server.example" => Some("100012".to_string()),
            "carol+work@server.example" => Some("100013".to_string()),
            _ => None,
        })
}

/// [`own_account`] with its [`Level`], as the listener looks it up.
fn own_leveled(address: &str) -> Option<(String, Level)> {
    leveled(|| Ok(own_account(address, "")), || Ok(own_account(address, "+"))).unwrap()
}

#[test]
fn an_all_in_one_node_finds_its_own_accounts_first_then_the_paired_nodes_tagged() {
    let copies = all_in_one_copies();
    let answer = |address: &str| choose(address, own_leveled(address), leveled_lookup(&copies, address, "+"));
    assert_eq!(answer("sales@server.example"), RcptAnswer::Local("100".to_string()));
    assert_eq!(
        answer("sales+news@server.example"),
        RcptAnswer::Local("100".to_string())
    );
    assert_eq!(
        answer("ann@desk.example"),
        RcptAnswer::Local("node:node-a:100".to_string()),
        "the paired node's account 100 is not the server's account 100"
    );
    assert_eq!(answer("nobody@server.example"), RcptAnswer::Unknown);
    match answer("both@server.example") {
        RcptAnswer::TempFail(text) => {
            assert!(text.contains("both@server.example"), "{text}");
            assert!(text.contains("100011") && text.contains("node-a"), "{text}");
        }
        other => panic!("an address both sides have is refused: {other:?}"),
    }
    // both sides only without the plus tag: refused too
    assert!(matches!(answer("both+x@server.example"), RcptAnswer::TempFail(_)));

    // the paired node away: its addresses wait, the server's own are taken
    let mut away = all_in_one_copies();
    away.active.clear();
    assert_eq!(
        choose("ann@desk.example", None, leveled_lookup(&away, "ann@desk.example", "+")),
        RcptAnswer::TempFail("the local node of the address is not paired now".to_string())
    );
    assert_eq!(
        choose(
            "sales@server.example",
            own_leveled("sales@server.example"),
            leveled_lookup(&away, "sales@server.example", "+")
        ),
        RcptAnswer::Local("100".to_string())
    );
    // the paired node's set not sent whole yet: an address of neither side waits
    let mut loading = all_in_one_copies();
    loading.loaded.clear();
    assert!(matches!(
        choose(
            "nobody@server.example",
            None,
            leveled_lookup(&loading, "nobody@server.example", "+")
        ),
        RcptAnswer::TempFail(_)
    ));
}

#[test]
fn an_exact_match_on_one_side_wins_over_a_match_without_the_plus_tag_on_the_other() {
    let values = vec![
        account("node-a", "200", "bob+work@server.example", "EakHosted", ""),
        account("node-a", "201", "carol@server.example", "EakHosted", ""),
    ];
    let copies = Copies {
        built_at: Some(1),
        loaded: HashSet::from(["node-a".to_string()]),
        nodes: build(&values),
        active: HashSet::from(["node-a".to_string()]),
    };
    let answer = |address: &str| choose(address, own_leveled(address), leveled_lookup(&copies, address, "+"));
    // the server has bob@, the paired node bob+work@
    assert_eq!(
        answer("bob+work@server.example"),
        RcptAnswer::Local("node:node-a:200".to_string()),
        "the paired node's exact address"
    );
    assert_eq!(answer("bob@server.example"), RcptAnswer::Local("100012".to_string()));
    assert_eq!(
        answer("bob+home@server.example"),
        RcptAnswer::Local("100012".to_string()),
        "only the server's bob@ matches without the tag"
    );
    // the paired node has carol@, the server carol+work@
    assert_eq!(
        answer("carol+work@server.example"),
        RcptAnswer::Local("100013".to_string()),
        "the server's exact address"
    );
    assert_eq!(
        answer("carol+home@server.example"),
        RcptAnswer::Local("node:node-a:201".to_string())
    );
    assert_eq!(leveled_lookup(&copies, "bob+work@server.example", "+").1, Level::Exact);
    assert_eq!(
        leveled_lookup(&copies, "carol+x@server.example", "+").1,
        Level::Stripped
    );
}

#[test]
fn a_message_to_both_sides_is_split_by_side_and_the_queued_ids_are_bare() {
    let group = |account: &str, address: &str| Recipients {
        email_account_id: account.to_string(),
        addresses: vec![address.to_string()],
        ..Recipients::default()
    };
    let (own, link) = split_sides(vec![
        group("100", "sales@server.example"),
        group("node:node-a:100", "ann@desk.example"),
        group("bounce:ITEM1", "bounce+ITEM1@bounce.server.example"),
        group("100011", "help@server.example"),
    ]);
    let ids = |groups: &[Recipients]| {
        groups
            .iter()
            .map(|group| group.email_account_id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&own), ["100", "100011"]);
    assert_eq!(ids(&link), ["node:node-a:100", "bounce:ITEM1"]);
    assert_eq!(side("node:node-a:100"), Side::Paired);
    assert_eq!(side("bounce:ITEM1"), Side::Bounce);
    assert_eq!(side("100"), Side::Own);
    // the queued item's conversation is the account's id on the paired node, as on an edge node
    assert_eq!(bare_account("node:node-a:100"), "100");
    assert_eq!(bare_account("100"), "100");
    assert_eq!(node_account("node-a", "100"), "node:node-a:100");
}
