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
use crate::context::ExecutionContext;
use crate::edge::replica::{apply_batch, Batch, Change};

fn open() -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    (ctx, facade)
}

fn create(facade: &EntityFacade, entity: &str, fields: &[(&str, &str)]) {
    let mut row = facade.make_value(entity).unwrap();
    for (field, value) in fields {
        row.set(*field, Value::string(*value));
    }
    row.create(facade).unwrap();
}

fn paired(facade: &EntityFacade, node: &str, setup: &str) {
    create(
        facade,
        EDGE_CLIENT,
        &[
            ("pairedNodeId", node),
            ("keyFingerprint", "ab"),
            ("statusId", "EdgeClientActive"),
            ("setupJson", setup),
        ],
    );
}

/// A paired node's full send of `set`, as the link applies it.
fn replicate(facade: &EntityFacade, node: &str, set: &str, changes: Vec<Change>) {
    let batch = Batch {
        set_name: set.to_string(),
        epoch: "e1".to_string(),
        base_seq: 0,
        seq: 1,
        snapshot: true,
        snapshot_end: true,
        changes,
    };
    apply_batch(facade, node, &batch).unwrap();
}

fn change(entity: &str, pk: serde_json::Value, fields: serde_json::Value) -> Change {
    Change {
        entity_name: entity.to_string(),
        pk_json: pk.to_string(),
        fields_json: Some(fields.to_string()),
    }
}

#[test]
fn the_message_names_the_item_and_its_holder() {
    assert_eq!(
        claimed_message(
            ClaimKind::MailAddress,
            " ann@example.com ",
            "the server's EmailAccount A1"
        ),
        "mail address ann@example.com is held by the server's EmailAccount A1"
    );
    assert_eq!(
        url_host("https://Chat.Example.com:8443/x?y"),
        Some("chat.example.com".to_string())
    );
    assert_eq!(url_host("not a url"), None);
}

#[test]
fn the_server_holds_its_receiving_accounts_addresses_bridges_and_numbers() {
    let (ctx, facade) = open();
    let factory = ctx.factory();
    create(
        &facade,
        ACCOUNT,
        &[
            ("emailAccountId", "A1"),
            ("emailAddress", "Ann@Example.com"),
            ("accountKindEnumId", "EakHosted"),
        ],
    );
    create(
        &facade,
        ACCOUNT,
        &[
            ("emailAccountId", "A2"),
            ("emailAddress", "gone@example.com"),
            ("accountKindEnumId", "EakHosted"),
            ("statusId", "EasRemoved"),
        ],
    );
    create(
        &facade,
        ACCOUNT,
        &[
            ("emailAccountId", "A3"),
            ("emailAddress", "me@elsewhere.com"),
            ("accountKindEnumId", "EakExternal"),
        ],
    );
    create(
        &facade,
        IDENTITY,
        &[
            ("emailIdentityId", "I1"),
            ("emailAccountId", "A1"),
            ("emailAddress", "sales@example.com"),
        ],
    );
    create(&facade, BRIDGE, &[("chatBridgeId", "OWN_BRIDGE")]);
    create(&facade, PHONE_NUMBER, &[("phoneNumber", "+15551230001")]);
    let own = |kind, key| own_claim(factory, &facade, kind, key).unwrap();
    assert_eq!(
        own(ClaimKind::MailAddress, "ann@example.com").as_deref(),
        Some("the server's EmailAccount A1")
    );
    assert_eq!(
        own(ClaimKind::MailAddress, "SALES@example.com").as_deref(),
        Some("the server's EmailIdentity I1 of EmailAccount A1")
    );
    assert_eq!(own(ClaimKind::MailAddress, "gone@example.com"), None, "removed");
    assert_eq!(
        own(ClaimKind::MailAddress, "me@elsewhere.com"),
        None,
        "not delivered here"
    );
    assert_eq!(own(ClaimKind::MailAddress, "bob@example.com"), None);
    assert_eq!(
        own(ClaimKind::ChatBridge, "OWN_BRIDGE").as_deref(),
        Some("the server's ChatBridge OWN_BRIDGE")
    );
    assert_eq!(own(ClaimKind::ChatBridge, "OTHER"), None);
    assert_eq!(
        own(ClaimKind::PhoneNumber, "+15551230001").as_deref(),
        Some("the server's PhoneNumber +15551230001")
    );
    assert_eq!(own(ClaimKind::PhoneNumber, "+15551230002"), None);
    assert_eq!(own(ClaimKind::HostName, " "), None);
}

#[test]
fn a_paired_node_holds_what_its_copies_and_configure_name() {
    let (_ctx, facade) = open();
    paired(
        &facade,
        "local-a",
        r#"{"xmppDomain":"Chat.Local.example.","sipDomain":"sip.local.example"}"#,
    );
    create(
        &facade,
        EDGE_HOST,
        &[
            ("hostName", "www.local.example"),
            ("pairedNodeId", "local-a"),
            ("modeEnumId", crate::edge::HOST_PASSTHROUGH),
        ],
    );
    replicate(
        &facade,
        "local-a",
        MAIL_SET,
        vec![
            change(
                ACCOUNT,
                serde_json::json!({"emailAccountId": "LA1"}),
                serde_json::json!({"emailAddress": "Bob@Local.example", "accountKindEnumId": "EakHosted"}),
            ),
            change(
                IDENTITY,
                serde_json::json!({"emailIdentityId": "LI1"}),
                serde_json::json!({"emailAddress": "info@local.example", "emailAccountId": "LA1"}),
            ),
        ],
    );
    replicate(
        &facade,
        "local-a",
        CHAT_SET,
        vec![change(
            BRIDGE,
            serde_json::json!({"chatBridgeId": "LOCAL_BRIDGE"}),
            serde_json::json!({"typeEnumId": "CbtSms"}),
        )],
    );
    replicate(
        &facade,
        "local-a",
        FALLBACKS_SET,
        vec![change(
            FALLBACK,
            serde_json::json!({"phoneNumber": "+15551239999"}),
            serde_json::json!({"voicemailFlag": "Y"}),
        )],
    );
    let held = |kind, key| replica_claim(&facade, kind, key).unwrap();
    let local = Some("local-a".to_string());
    assert_eq!(held(ClaimKind::HostName, "WWW.local.example."), local);
    assert_eq!(held(ClaimKind::HostName, "api.local.example"), None);
    assert_eq!(held(ClaimKind::MailAddress, "bob@local.example"), local);
    assert_eq!(held(ClaimKind::MailAddress, "INFO@local.example"), local);
    assert_eq!(held(ClaimKind::MailAddress, "ann@local.example"), None);
    assert_eq!(held(ClaimKind::ChatBridge, "LOCAL_BRIDGE"), local);
    assert_eq!(held(ClaimKind::ChatBridge, "OWN_BRIDGE"), None);
    assert_eq!(held(ClaimKind::PhoneNumber, "+15551239999"), local);
    assert_eq!(held(ClaimKind::PhoneNumber, "+15551230001"), None);
    assert_eq!(held(ClaimKind::XmppDomain, "chat.local.example"), local);
    assert_eq!(held(ClaimKind::SipDomain, "SIP.local.example"), local);
    assert_eq!(held(ClaimKind::SipDomain, "sip.example.com"), None);

    // a revoked node's copies are kept for a re-pair, but claim nothing; its host names stay reserved
    facade
        .find(EDGE_CLIENT)
        .unwrap()
        .eq("pairedNodeId", Value::string("local-a"))
        .disable_authz()
        .update_all([("statusId".to_string(), Value::string("EdgeClientRevoked"))])
        .unwrap();
    assert_eq!(held(ClaimKind::HostName, "www.local.example"), local);
    assert_eq!(held(ClaimKind::MailAddress, "bob@local.example"), None);
    assert_eq!(held(ClaimKind::ChatBridge, "LOCAL_BRIDGE"), None);
    assert_eq!(held(ClaimKind::PhoneNumber, "+15551239999"), None);
    assert_eq!(held(ClaimKind::XmppDomain, "chat.local.example"), None);
}

#[test]
fn a_configure_loses_the_servers_own_items_and_keeps_a_shared_mail_domain() {
    let (ctx, facade) = open();
    let factory = ctx.factory();
    create(
        &facade,
        ACCOUNT,
        &[
            ("emailAccountId", "A1"),
            ("emailAddress", "ann@example.com"),
            ("accountKindEnumId", "EakHosted"),
        ],
    );
    create(&facade, BRIDGE, &[("chatBridgeId", "OWN_BRIDGE")]);
    let mut setup = serde_json::json!({
        "mailDomains": ["Example.com", "local.example"],
        "chatBridgeIds": ["OWN_BRIDGE", "LOCAL_BRIDGE"],
        "passSites": [],
        "xmppDomain": "chat.local.example",
    });
    let found = check_setup_claims(factory, &facade, &mut setup);
    let refused: Vec<(&str, ClaimKind, &str, &str)> = found
        .refused
        .iter()
        .map(|item| (item.field, item.kind, item.key.as_str(), item.holder.as_str()))
        .collect();
    assert_eq!(
        refused,
        vec![(
            "chatBridgeIds",
            ClaimKind::ChatBridge,
            "OWN_BRIDGE",
            "the server's ChatBridge OWN_BRIDGE"
        )]
    );
    assert_eq!(setup["chatBridgeIds"], serde_json::json!(["LOCAL_BRIDGE"]));
    assert_eq!(setup["xmppDomain"], "chat.local.example");
    // a mail domain is split by address: the server's account in it is noted, not refused
    assert_eq!(
        setup["mailDomains"],
        serde_json::json!(["Example.com", "local.example"])
    );
    assert_eq!(found.shared_domains, vec![("example.com".to_string(), 1)]);
}

#[test]
fn an_all_in_one_node_reserves_the_domains_of_its_own_receiving_accounts() {
    let (ctx, facade) = open();
    let factory = ctx.factory();
    crate::edge::conf::settings_as(factory, crate::edge::NodeRole::AllInOne);
    create(
        &facade,
        ACCOUNT,
        &[
            ("emailAccountId", "A1"),
            ("emailAddress", "ann@Example.com"),
            ("accountKindEnumId", "EakHosted"),
        ],
    );
    create(
        &facade,
        ACCOUNT,
        &[
            ("emailAccountId", "A2"),
            ("emailAddress", "old@removed.example"),
            ("accountKindEnumId", "EakShared"),
            ("statusId", "EasRemoved"),
        ],
    );
    create(
        &facade,
        ACCOUNT,
        &[
            ("emailAccountId", "A3"),
            ("emailAddress", "me@imap.example"),
            ("accountKindEnumId", "EakExternal"),
        ],
    );
    let names = own_host_names(factory);
    assert!(names.contains(&"example.com".to_string()), "{names:?}");
    assert!(!names
        .iter()
        .any(|name| name == "removed.example" || name == "imap.example"));
    let own = |key| own_claim(factory, &facade, ClaimKind::HostName, key).unwrap();
    assert_eq!(
        own("mta-sts.example.com").as_deref(),
        Some("the server's host name example.com")
    );
    assert_eq!(
        own("example.com").as_deref(),
        Some("the server's host name example.com")
    );
    assert_eq!(own("www.local.example"), None);
}
