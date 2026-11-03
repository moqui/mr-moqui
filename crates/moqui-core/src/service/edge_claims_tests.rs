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
use crate::context::ExecutionContextFactory;
use crate::edge::replica::{apply_batch, Batch, Change};
use crate::edge::NodeRole;

fn open(role: NodeRole) -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    // the edge conf as the factory keeps it, for a node of `role`
    ctx.factory().extension(|_| crate::edge::conf::EdgeSettings {
        role,
        ..Default::default()
    });
    (ctx, facade)
}

fn create(facade: &EntityFacade, entity: &str, fields: &[(&str, &str)]) {
    let mut row = facade.make_value(entity).unwrap();
    for (field, value) in fields {
        row.set(*field, Value::string(*value));
    }
    row.create(facade).unwrap();
}

fn paired_copies(facade: &EntityFacade) {
    create(
        facade,
        "moqui.edge.EdgeClient",
        &[
            ("pairedNodeId", "local-a"),
            ("keyFingerprint", "ab"),
            ("statusId", "EdgeClientActive"),
        ],
    );
    let batch = |set: &str, entity: &str, pk: &str, fields: &str| Batch {
        set_name: set.to_string(),
        epoch: "e1".to_string(),
        base_seq: 0,
        seq: 1,
        snapshot: true,
        snapshot_end: true,
        changes: vec![Change {
            entity_name: entity.to_string(),
            pk_json: pk.to_string(),
            fields_json: Some(fields.to_string()),
        }],
    };
    apply_batch(
        facade,
        "local-a",
        &batch(
            "mailRecipients",
            ACCOUNT,
            r#"{"emailAccountId":"LA1"}"#,
            r#"{"emailAddress":"bob@local.example","accountKindEnumId":"EakHosted"}"#,
        ),
    )
    .unwrap();
    apply_batch(
        facade,
        "local-a",
        &batch(
            "chatBridges",
            "moqui.chat.ChatBridge",
            r#"{"chatBridgeId":"LOCAL_BRIDGE"}"#,
            r#"{"typeEnumId":"CbtSms"}"#,
        ),
    )
    .unwrap();
}

fn check(facade: &EntityFacade, pairs: &[(&str, &str)]) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = pairs
        .iter()
        .map(|(name, value)| (name.to_string(), Value::string(*value)))
        .collect();
    check_replica_claim(&parameters, facade).unwrap()
}

#[test]
fn the_servers_own_item_a_paired_node_holds_is_refused_naming_the_node() {
    let (ctx, facade) = open(NodeRole::AllInOne);
    let _entered = ctx.enter();
    paired_copies(&facade);
    create(
        &facade,
        ACCOUNT,
        &[
            ("emailAccountId", "OWN1"),
            ("emailAddress", "Bob@Local.example"),
            ("accountKindEnumId", "EakHosted"),
        ],
    );
    create(
        &facade,
        ACCOUNT,
        &[
            ("emailAccountId", "EXT1"),
            ("emailAddress", "bob@local.example"),
            ("accountKindEnumId", "EakExternal"),
        ],
    );
    create(
        &facade,
        IDENTITY,
        &[
            ("emailIdentityId", "OWN_ID"),
            ("emailAccountId", "OWN1"),
            ("emailAddress", "bob@local.example"),
        ],
    );

    let out = check(&facade, &[("itemType", "EmailAccount"), ("itemId", "OWN1")]);
    assert_eq!(out.get("pairedNodeId").and_then(Value::as_str), Some("local-a"));
    let errors: Vec<String> = ctx.message().errors().iter().map(ToString::to_string).collect();
    assert_eq!(
        errors,
        vec!["mail address Bob@Local.example is held by paired node local-a".to_string()]
    );
    ctx.message().clear_errors();

    let out = check(&facade, &[("itemType", "EmailIdentity"), ("itemId", "OWN_ID")]);
    assert_eq!(out.get("pairedNodeId").and_then(Value::as_str), Some("local-a"));
    ctx.message().clear_errors();

    // an external account reads mail elsewhere: its address is no claim
    assert!(check(&facade, &[("itemType", "EmailAccount"), ("itemId", "EXT1")]).is_empty());
    // a probe that does not refuse
    let out = check(
        &facade,
        &[
            ("itemType", "ChatBridge"),
            ("itemId", "LOCAL_BRIDGE"),
            ("refuse", "false"),
        ],
    );
    assert_eq!(out.get("pairedNodeId").and_then(Value::as_str), Some("local-a"));
    assert!(ctx.message().errors().is_empty());
    assert!(check(&facade, &[("itemType", "ChatBridge"), ("itemId", "OWN_BRIDGE")]).is_empty());
    assert!(check(&facade, &[("claimKind", "PhoneNumber"), ("itemKey", "+15551230001")]).is_empty());
}

#[test]
fn a_node_that_is_not_all_in_one_checks_nothing() {
    for role in [NodeRole::Normal, NodeRole::Edge] {
        let (ctx, facade) = open(role);
        let _entered = ctx.enter();
        paired_copies(&facade);
        assert!(check(&facade, &[("itemType", "ChatBridge"), ("itemId", "LOCAL_BRIDGE")]).is_empty());
        assert!(ctx.message().errors().is_empty());
    }
}
