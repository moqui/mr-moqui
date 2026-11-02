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
