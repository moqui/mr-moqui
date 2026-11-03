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

use std::collections::BTreeMap;

use super::super::{apply_batch, get_value, Batch, BatchAnswer, Change};
use super::*;
use crate::edge::NodeRole;

const ACCOUNT: &str = "moqui.basic.email.EmailAccount";
const BRIDGE: &str = "moqui.chat.ChatBridge";
const NODE: &str = "local-a";

fn open(role: NodeRole) -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    crate::edge::conf::settings_as(ctx.factory(), role);
    (ctx, facade)
}

fn create(facade: &EntityFacade, entity: &str, fields: &[(&str, &str)]) {
    let mut row = facade.make_value(entity).unwrap();
    for (field, value) in fields {
        row.set(*field, Value::string(*value));
    }
    row.create(facade).unwrap();
}

fn change(entity: &str, pk: serde_json::Value, fields: Option<serde_json::Value>) -> Change {
    Change {
        entity_name: entity.to_string(),
        pk_json: canonical(&pk),
        fields_json: fields.map(|fields| fields.to_string()),
    }
}

fn batch(set: &str, seq: u64, snapshot: bool, changes: Vec<Change>) -> Batch {
    Batch {
        set_name: set.to_string(),
        epoch: "e1".to_string(),
        base_seq: if snapshot { 0 } else { seq - 1 },
        seq,
        snapshot,
        snapshot_end: snapshot,
        changes,
    }
}

fn copy(facade: &EntityFacade, set: &str, entity: &str, field: &str, id: &str) -> bool {
    let pk = BTreeMap::from([(field.to_string(), id.to_string())]);
    get_value(facade, Some(NODE), set, entity, &pk).unwrap().is_some()
}

fn own_items(facade: &EntityFacade) {
    create(
        facade,
        ACCOUNT,
        &[
            ("emailAccountId", "OWN_ACCOUNT"),
            ("emailAddress", "ann@example.com"),
            ("accountKindEnumId", "EakHosted"),
        ],
    );
    create(facade, BRIDGE, &[("chatBridgeId", "SHARED_ID")]);
}

fn replicate(facade: &EntityFacade) {
    let mail = batch(
        "mailRecipients",
        1,
        true,
        vec![
            change(
                ACCOUNT,
                serde_json::json!({"emailAccountId": "LA1"}),
                Some(serde_json::json!({"emailAddress": "Ann@Example.com", "accountKindEnumId": "EakHosted"})),
            ),
            change(
                ACCOUNT,
                serde_json::json!({"emailAccountId": "LA2"}),
                Some(serde_json::json!({"emailAddress": "bob@example.com", "accountKindEnumId": "EakHosted"})),
            ),
        ],
    );
    assert_eq!(apply_batch(facade, NODE, &mail).unwrap(), BatchAnswer::Applied);
    let chat = batch(
        "chatBridges",
        1,
        true,
        vec![
            change(
                BRIDGE,
                serde_json::json!({"chatBridgeId": "SHARED_ID"}),
                Some(serde_json::json!({"typeEnumId": "CbtSms"})),
            ),
            change(
                BRIDGE,
                serde_json::json!({"chatBridgeId": "LOCAL_BRIDGE"}),
                Some(serde_json::json!({"typeEnumId": "CbtSms"})),
            ),
        ],
    );
    assert_eq!(apply_batch(facade, NODE, &chat).unwrap(), BatchAnswer::Applied);
}

#[test]
fn a_record_claiming_the_servers_own_item_is_skipped_kept_and_applied_once_the_claim_goes() {
    let (ctx, facade) = open(NodeRole::AllInOne);
    let _entered = ctx.enter();
    own_items(&facade);
    replicate(&facade);

    // the rest of each batch went in; the two claims did not
    assert!(copy(&facade, "mailRecipients", ACCOUNT, "emailAccountId", "LA2"));
    assert!(!copy(&facade, "mailRecipients", ACCOUNT, "emailAccountId", "LA1"));
    assert!(copy(&facade, "chatBridges", BRIDGE, "chatBridgeId", "LOCAL_BRIDGE"));
    assert!(!copy(&facade, "chatBridges", BRIDGE, "chatBridgeId", "SHARED_ID"));
    let found = list_conflicts(&facade, Some(NODE)).unwrap();
    let mut seen: Vec<(String, String, String)> = found
        .iter()
        .map(|conflict| {
            (
                conflict.claim_kind.clone(),
                conflict.item_key.clone(),
                conflict.owner_text.clone(),
            )
        })
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        vec![
            (
                "ChatBridge".to_string(),
                "SHARED_ID".to_string(),
                "the server's ChatBridge SHARED_ID".to_string()
            ),
            (
                "MailAddress".to_string(),
                "Ann@Example.com".to_string(),
                "the server's EmailAccount OWN_ACCOUNT".to_string()
            ),
        ]
    );
    assert!(found.iter().all(|conflict| conflict.last_date.is_some()));
    // the paired node does not hold what was skipped
    assert_eq!(
        claims::replica_claim(&facade, ClaimKind::ChatBridge, "SHARED_ID").unwrap(),
        None
    );

    // the server's bridge goes: Check again applies the record kept
    facade
        .find(BRIDGE)
        .unwrap()
        .eq("chatBridgeId", Value::string("SHARED_ID"))
        .delete_all()
        .unwrap();
    assert_eq!(recheck_conflicts(ctx.factory(), &facade, Some(NODE)).unwrap(), 1);
    assert!(copy(&facade, "chatBridges", BRIDGE, "chatBridgeId", "SHARED_ID"));
    assert_eq!(list_conflicts(&facade, Some(NODE)).unwrap().len(), 1);

    // a later batch that deletes the account clears its conflict
    let delete = batch(
        "mailRecipients",
        2,
        false,
        vec![change(ACCOUNT, serde_json::json!({"emailAccountId": "LA1"}), None)],
    );
    assert_eq!(apply_batch(&facade, NODE, &delete).unwrap(), BatchAnswer::Applied);
    assert!(list_conflicts(&facade, Some(NODE)).unwrap().is_empty());
}

#[test]
fn a_full_send_without_the_record_drops_its_conflict() {
    let (ctx, facade) = open(NodeRole::AllInOne);
    let _entered = ctx.enter();
    own_items(&facade);
    replicate(&facade);
    let mut again = batch("chatBridges", 1, true, Vec::new());
    again.epoch = "e2".to_string();
    assert_eq!(apply_batch(&facade, NODE, &again).unwrap(), BatchAnswer::Applied);
    let left: Vec<String> = list_conflicts(&facade, Some(NODE))
        .unwrap()
        .into_iter()
        .map(|conflict| conflict.set_name)
        .collect();
    assert_eq!(left, vec!["mailRecipients".to_string()]);
}

#[test]
fn a_dedicated_edge_node_applies_every_record() {
    let (ctx, facade) = open(NodeRole::Edge);
    let _entered = ctx.enter();
    own_items(&facade);
    replicate(&facade);
    assert!(copy(&facade, "mailRecipients", ACCOUNT, "emailAccountId", "LA1"));
    assert!(copy(&facade, "chatBridges", BRIDGE, "chatBridgeId", "SHARED_ID"));
    assert!(list_conflicts(&facade, None).unwrap().is_empty());
    assert_eq!(recheck_conflicts(ctx.factory(), &facade, None).unwrap(), 0);
}

#[test]
fn a_records_claim_is_its_address_bridge_or_number() {
    let pk = serde_json::json!({"emailIdentityId": "I1"});
    let fields = serde_json::json!({"emailAddress": " info@local.example "});
    assert_eq!(
        claims::copy_claim("moqui.basic.email.EmailIdentity", &pk, &fields),
        Some((ClaimKind::MailAddress, "info@local.example".to_string()))
    );
    assert_eq!(
        claims::copy_claim(
            "moqui.call.PhoneNumberFallback",
            &serde_json::json!({"phoneNumber": "+15551230001"}),
            &serde_json::json!({})
        ),
        Some((ClaimKind::PhoneNumber, "+15551230001".to_string()))
    );
    assert_eq!(
        claims::copy_claim(
            "moqui.call.VoiceTrunk",
            &serde_json::json!({"voiceTrunkId": "T1"}),
            &serde_json::json!({})
        ),
        None
    );
    assert_eq!(
        claims::copy_claim("moqui.basic.email.EmailServer", &serde_json::json!({}), &fields),
        None
    );
    for kind in [ClaimKind::HostName, ClaimKind::SipDomain, ClaimKind::PhoneNumber] {
        assert_eq!(ClaimKind::from_id(kind.id()), Some(kind));
    }
    assert_eq!(ClaimKind::from_id("Other"), None);
}
